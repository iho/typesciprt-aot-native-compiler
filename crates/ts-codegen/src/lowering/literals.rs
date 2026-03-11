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

        // If the array has spread elements, use push-based construction.
        let has_spread = array.elements.iter().any(|e| e.is_spread());

        if has_spread {
            // Allocate empty array, then push/push_all each element.
            let zero = self.lower_numeric_literal(0, block)?;
            let arr_val: Value<'c, 'b> = block
                .append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
                    &[zero],
                    &[i64_type],
                    self.loc,
                ))
                .result(0)?
                .into();

            for elem in &array.elements {
                use oxc_ast::ast::ArrayExpressionElement;
                match elem {
                    ArrayExpressionElement::SpreadElement(spread) => {
                        let (val_opt, nb) = self.lower_expression(&spread.argument, block, region, scope)?;
                        block = nb;
                        let val = val_opt
                            .ok_or_else(|| anyhow::anyhow!("spread element produced no value"))?;
                        let val_i64 = self.ensure_i64(val, block)?;
                        block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push_all"),
                            &[arr_val, val_i64],
                            &[],
                            self.loc,
                        ));
                        block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                            &[val_i64],
                            &[],
                            self.loc,
                        ));
                    }
                    _ => {
                        let Some(expr) = elem.as_expression() else { continue };
                        let (val_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                        block = nb;
                        let val = val_opt
                            .ok_or_else(|| anyhow::anyhow!("array element produced no value"))?;
                        let val_i64 = self.ensure_i64(val, block)?;
                        // push returns new length — discard it
                        let _len: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push"),
                            &[arr_val, val_i64],
                            &[i64_type],
                            self.loc,
                        )).result(0)?.into();
                        block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                            &[val_i64],
                            &[],
                            self.loc,
                        ));
                    }
                }
            }

            return Ok((Some(arr_val), block));
        }

        // No spread: pre-allocate by count and use ts_arr_set by index (faster).
        let n = array.elements.len();
        let n_val = self.lower_numeric_literal(n as i64, block)?;

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

        for (i, elem) in array.elements.iter().enumerate() {
            let Some(expr) = elem.as_expression() else { continue };
            let (val_opt, nb) = self.lower_expression(expr, block, region, scope)?;
            block = nb;
            let val = val_opt
                .ok_or_else(|| anyhow::anyhow!("array element produced no value"))?;

            let val_i64 = self.ensure_i64(val, block)?;
            let idx_val = self.lower_numeric_literal(i as i64, block)?;

            block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_arr_set"),
                &[arr_val, idx_val, val_i64],
                &[],
                self.loc,
            ));

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


    /// Lower `obj[key]` — generic computed member read using `ts_val_get_key`.
    /// Works for arrays (integer index), objects (string key), Maps, and strings (char-at index).
    pub(super) fn lower_computed_member_expression<'b>(
        &mut self,
        member: &oxc_ast::ast::ComputedMemberExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let i64_type = self.i64_type();

        let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
        block = nb;
        let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("computed member: object expression produced no value"))?;
        let obj_i64 = self.ensure_i64(obj, block)?;

        let (key_opt, nb) = self.lower_expression(&member.expression, block, region, scope)?;
        block = nb;
        let key = key_opt.ok_or_else(|| anyhow::anyhow!("computed member: key expression produced no value"))?;
        let key_i64 = self.ensure_i64(key, block)?;

        // %val = ts_val_get_key(%obj_i64, %key_i64) : (i64, i64) -> i64
        let val: Value<'c, 'b> = block
            .append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_val_get_key"),
                &[obj_i64, key_i64],
                &[i64_type],
                self.loc,
            ))
            .result(0)?
            .into();

        // ARC: Release the object and key after the call.
        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[key_i64], &[], self.loc));

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

            // Handle spread: { ...src } → ts_obj_merge(dst, src)
            if let ObjectPropertyKind::SpreadProperty(spread) = prop {
                let (src_opt, nb) = self.lower_expression(&spread.argument, block, region, scope)?;
                block = nb;
                if let Some(src) = src_opt {
                    let src_i64 = self.ensure_i64(src, block)?;
                    block.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_obj_merge"),
                        &[obj_val, src_i64],
                        &[],
                        self.loc,
                    ));
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[src_i64], &[], self.loc,
                    ));
                }
                continue;
            }

            let ObjectPropertyKind::ObjectProperty(p) = prop else {
                continue;
            };

            let (val_opt, nb) = self.lower_expression(&p.value, block, region, scope)?;
            block = nb;
            let val = val_opt.ok_or_else(|| anyhow::anyhow!("object property value produced no value"))?;
            let val_i64 = self.ensure_i64(val, block)?;

            if let Some(key_str) = p.key.static_name() {
                // Static key: { foo: val } or { "foo": val } or { 0: val }
                let key_ptr = self.get_string_ptr(&key_str, block)?;
                block.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
                    &[obj_val, key_ptr, val_i64],
                    &[],
                    self.loc,
                ));
            } else {
                // Computed key: { [expr]: val }
                // Try to extract the key as an identifier reference (most common computed case).
                let key_val: Option<Value<'c, 'b>> = match &p.key {
                    oxc_ast::ast::PropertyKey::Identifier(id_ref) => {
                        // Variable reference key
                        if let Some(&v) = scope.get(id_ref.name.as_str()) {
                            let v_i64 = self.ensure_i64(v, block)?;
                            block.append_operation(func::call(
                                self.ctx,
                                FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"),
                                &[v_i64], &[], self.loc,
                            ));
                            Some(v_i64)
                        } else {
                            tracing::warn!("computed property key: undefined variable {}", id_ref.name);
                            None
                        }
                    }
                    _ => {
                        tracing::debug!("skipping unsupported computed property key type");
                        None
                    }
                };

                if let Some(key_i64) = key_val {
                    block.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set_val_key"),
                        &[obj_val, key_i64, val_i64],
                        &[],
                        self.loc,
                    ));
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[key_i64], &[], self.loc,
                    ));
                }
            }
        }

        Ok((Some(obj_val), block))
    }
}
