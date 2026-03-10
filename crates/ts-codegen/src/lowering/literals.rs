use super::*;

impl<'c, 'm> Lowerer<'c, 'm> {

    // ── Literals ──────────────────────────────────────────────────────────

    pub(super) fn lower_numeric_literal<'b>(&self, value: i64, block: BlockRef<'c, 'b>) -> Result<Value<'c, 'b>> {
        Ok(block
            .append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(self.i32_type(), value).into(),
                self.loc,
            ))
            .result(0)?
            .into())
    }

    pub(super) fn lower_boolean_literal<'b>(&self, value: bool, block: BlockRef<'c, 'b>) -> Result<Value<'c, 'b>> {
        Ok(block
            .append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(self.i1_type(), if value { 1 } else { 0 }).into(),
                self.loc,
            ))
            .result(0)?
            .into())
    }

    // ── Array literals ────────────────────────────────────────────────────

    /// Lower `[e0, e1, …]` to a stack-allocated i32 array.
    ///
    /// Layout in memory:  `[i32 length | i32 e0 | i32 e1 | …]`
    /// The returned value is a `!llvm.ptr` to the first element (the length).
    /// Lower `[e0, e1, …]` to a heap-allocated TsArray with ARC.
    pub(super) fn lower_array_expression<'b>(
        &mut self,
        array: &oxc_ast::ast::ArrayExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let i64_type = self.i64_type();

        let n = array.elements.len();
        let n_val = self.lower_numeric_literal(n as i64, block)?;

        // %arr = func.call @ts_arr_new(%n) : (i32) -> i64
        let arr_val: Value<'c, 'b> = block
            .append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
                &[n_val],
                &[i64_type],
                self.loc,
            ))
            .result(0)?
            .into();

        // Store each element.
        for (i, elem) in array.elements.iter().enumerate() {
            let Some(expr) = elem.as_expression() else { continue };
            let (val_opt, nb) = self.lower_expression(expr, block, region, scope)?;
            block = nb;
            let val = val_opt
                .ok_or_else(|| anyhow::anyhow!("array element produced no value"))?;
            
            let val_i64 = self.ensure_i64(val, block)?;
            let idx_val = self.lower_numeric_literal(i as i64, block)?;

            // func.call @ts_arr_set(%arr, %idx, %val_i64)
            block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_arr_set"),
                &[arr_val, idx_val, val_i64],
                &[],
                self.loc,
            ));
            
            // ARC: Release the temporary expression result (ts_arr_set retained it).
            block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[val_i64],
                &[],
                self.loc,
            ));
        }

        Ok((Some(arr_val), block))
    }


    /// Lower `arr[idx]` to an i32 load.
    /// Lower `arr[idx]` to a heap-allocated TsArray access.
    pub(super) fn lower_computed_member_expression<'b>(
        &mut self,
        member: &oxc_ast::ast::ComputedMemberExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let i64_type = self.i64_type();

        let (arr_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
        block = nb;
        let arr = arr_opt.ok_or_else(|| anyhow::anyhow!("array: object expression produced no value"))?;
        let arr_i64 = self.ensure_i64(arr, block)?;

        let (idx_opt, nb) = self.lower_expression(&member.expression, block, region, scope)?;
        block = nb;
        let idx = idx_opt.ok_or_else(|| anyhow::anyhow!("array: index expression produced no value"))?;
        let idx_i32 = self.ensure_i32(idx, block)?;

        // %val = func.call @ts_arr_get(%arr_i64, %idx_i32) : (i64, i32) -> i64
        let val: Value<'c, 'b> = block
            .append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
                &[arr_i64, idx_i32],
                &[i64_type],
                self.loc,
            ))
            .result(0)?
            .into();

        // ARC: Release the array and index (ts_arr_get returned an owned reference).
        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[arr_i64], &[], self.loc));
        let idx_i64 = self.ensure_i64(idx, block)?;
        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[idx_i64], &[], self.loc));

        Ok((Some(val), block))
    }


    // ── String literals ───────────────────────────────────────────────────

    /// Emit a `llvm.mlir.global` for the string (with null terminator) at
    /// module level, then return a `!llvm.ptr` pointing to the first byte.
    pub(super) fn get_string_ptr<'b>(
        &mut self,
        s: &str,
        block: BlockRef<'c, 'b>,
    ) -> Result<Value<'c, 'b>> {
        let name = format!("__ts_str_{}", self.string_count);
        self.string_count += 1;

        // Build null-terminated byte slice and treat it as a &str for MLIR
        // (mlirStringAttrGet uses (ptr, len) so embedded nulls are fine).
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0u8);
        let len = bytes.len() as u32;
        // SAFETY: MLIR receives (ptr, len), so any bytes are valid here.
        let content = unsafe { std::str::from_utf8_unchecked(&bytes) };

        let array_type = self.llvm_i8_array_type(len);
        let ptr_type   = self.llvm_ptr_type();
        let i32_type   = self.i32_type();

        let linkage  = Attribute::parse(self.ctx, "#llvm.linkage<internal>")
            .ok_or_else(|| anyhow::anyhow!("failed to parse #llvm.linkage<internal>"))?;
        let unit_attr = Attribute::parse(self.ctx, "unit")
            .ok_or_else(|| anyhow::anyhow!("failed to parse unit attribute"))?;

        // llvm.mlir.global internal constant @__ts_str_N("<bytes>") : !llvm.array<N x i8>
        let global_op = OperationBuilder::new("llvm.mlir.global", self.loc)
            .add_attributes(&[
                (Identifier::new(self.ctx, "sym_name"),    StringAttribute::new(self.ctx, &name).into()),
                (Identifier::new(self.ctx, "global_type"), TypeAttribute::new(array_type).into()),
                (Identifier::new(self.ctx, "linkage"),     linkage),
                (Identifier::new(self.ctx, "value"),       StringAttribute::new(self.ctx, content).into()),
                (Identifier::new(self.ctx, "addr_space"),  IntegerAttribute::new(i32_type, 0).into()),
                (Identifier::new(self.ctx, "constant"),    unit_attr),
            ])
            .add_regions([Region::new()])
            .build()?;
        self.module.body().append_operation(global_op);

        // %arr_ptr = llvm.mlir.addressof @__ts_str_N : !llvm.ptr
        let addr_op = OperationBuilder::new("llvm.mlir.addressof", self.loc)
            .add_attributes(&[(
                Identifier::new(self.ctx, "global_name"),
                FlatSymbolRefAttribute::new(self.ctx, &name).into(),
            )])
            .add_results(&[ptr_type])
            .build()?;
        let arr_ptr: Value<'c, 'b> = block.append_operation(addr_op).result(0)?.into();

        // %char_ptr = llvm.getelementptr inbounds %arr_ptr[0, 0] : (!llvm.ptr) -> !llvm.ptr
        let char_ptr: Value<'c, 'b> = block
            .append_operation(llvm::get_element_ptr(
                self.ctx,
                arr_ptr,
                DenseI32ArrayAttribute::new(self.ctx, &[0, 0]),
                array_type,
                ptr_type,
                self.loc,
            ))
            .result(0)?
            .into();

        Ok(char_ptr)
    }

    /// Lower a string literal to a heap-allocated TsString (TsVal).
    pub(super) fn lower_string_literal<'b>(
        &mut self,
        s: &str,
        block: BlockRef<'c, 'b>,
    ) -> Result<Value<'c, 'b>> {
        let char_ptr = self.get_string_ptr(s, block)?;
        let i64_type = self.i64_type();

        // %ts_str = func.call @ts_string_new(%char_ptr) : (!llvm.ptr) -> i64
        Ok(block
            .append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_string_new"),
                &[char_ptr],
                &[i64_type],
                self.loc,
            ))
            .result(0)?
            .into())
    }

    // ── Object literals ───────────────────────────────────────────────────

    pub(super) fn lower_object_expression<'b>(
        &mut self,
        obj: &oxc_ast::ast::ObjectExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let i64_type = self.i64_type();

        // %obj = func.call @ts_obj_new() : () -> i64
        let obj_val: Value<'c, 'b> = block
            .append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_obj_new"),
                &[],
                &[i64_type],
                self.loc,
            ))
            .result(0)?
            .into();

        for prop in &obj.properties {
            use oxc_ast::ast::ObjectPropertyKind;
            let ObjectPropertyKind::ObjectProperty(p) = prop else {
                continue; // Skip spreads for now
            };
            
            let key_str = match &p.key {
                oxc_ast::ast::PropertyKey::StaticIdentifier(id) => id.name.to_string(),
                _ => bail!("object literal: only static identifiers are supported as keys for now"),
            };

            // key_ptr = get_string_ptr(...)
            let key_ptr = self.get_string_ptr(&key_str, block)?;

            let (val_opt, nb) = self.lower_expression(&p.value, block, region, scope)?;
            block = nb;
            let val = val_opt.ok_or_else(|| anyhow::anyhow!("object property value produced no value"))?;
            let val_i64 = self.ensure_i64(val, block)?;

            // func.call @ts_obj_set(%obj, %key_ptr, %val_i64)
            block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
                &[obj_val, key_ptr, val_i64],
                &[],
                self.loc,
            ));
        }

        Ok((Some(obj_val), block))
    }
}
