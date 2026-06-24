use super::*;

impl<'c, 'm> Lowerer<'c, 'm> {
    // ── Chain expressions (optional chaining: obj?.prop / obj?.[idx]) ────

    pub(crate) fn lower_chain_expression<'b>(
        &mut self,
        chain: &ChainExpression<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        match &chain.expression {
            ChainElement::StaticMemberExpression(member) => {
                self.lower_optional_static_member(member, member.optional, block, region, scope)
            }
            ChainElement::ComputedMemberExpression(member) => {
                self.lower_optional_computed_member(member, member.optional, block, region, scope)
            }
            ChainElement::CallExpression(call) => {
                // If the callee is an optional member expression (obj?.method()),
                // we must null-guard the receiver before calling.
                let callee_optional = match &call.callee {
                    Expression::StaticMemberExpression(m) => m.optional,
                    Expression::ComputedMemberExpression(m) => m.optional,
                    _ => false,
                };
                if callee_optional {
                    self.lower_optional_call_expression(call, block, region, scope)
                } else {
                    self.lower_call_expression(call, block, region, scope)
                }
            }
            _ => {
                tracing::debug!("skipping unimplemented chain element");
                Ok((None, block))
            }
        }
    }

    /// Lower an optional method call: obj?.method(args).
    /// Evaluates the receiver, null-guards it, then calls the method if non-null.
    fn lower_optional_call_expression<'b>(
        &mut self,
        call: &oxc_ast::ast::CallExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        // Extract the receiver object from the optional member callee.
        let receiver_expr = match &call.callee {
            Expression::StaticMemberExpression(m) => &m.object,
            Expression::ComputedMemberExpression(m) => &m.object,
            _ => return self.lower_call_expression(call, block, region, scope),
        };

        // Evaluate receiver.
        let (recv_opt, nb) = self.lower_expression(receiver_expr, block, region, scope)?;
        block = nb;
        let recv = recv_opt.ok_or_else(|| anyhow::anyhow!("optional call: receiver no value"))?;
        let recv_i64 = self.ensure_i64(recv, block)?;

        // Null check.
        let is_null: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_is_nullish"),
            &[recv_i64],
            &[self.i32_type()],
            self.loc,
        )).result(0)?.into();
        let is_null_i1 = self.ensure_i1(is_null, block)?;

        // Normalize scope to i64.
        let scope_keys: Vec<String> = scope.keys().cloned().collect();
        for k in &scope_keys {
            let v64 = self.ensure_i64(scope[k], block)?;
            scope.insert(k.clone(), v64);
        }
        let orig_scope = scope.clone();

        // merge_block: (i64 result, ...scope_vals)
        let mut merge_arg_types = vec![(self.i64_type(), self.loc)];
        for _ in &scope_keys {
            merge_arg_types.push((self.i64_type(), self.loc));
        }
        let merge_block = region.append_block(Block::new(&merge_arg_types));
        let call_block  = region.append_block(Block::new(&[]));
        let null_block  = region.append_block(Block::new(&[]));

        let orig_vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| orig_scope[k]).collect();

        block.append_operation(cf::cond_br(
            self.ctx, is_null_i1, &null_block, &call_block, &[], &[], self.loc,
        ));

        // Null path: release recv, return UNDEFINED.
        null_block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[recv_i64], &[], self.loc,
        ));
        let undef_val: Value<'c, 'b> = null_block.append_operation(arith::constant(
            self.ctx,
            IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(),
            self.loc,
        )).result(0)?.into();
        let mut null_args = vec![undef_val];
        null_args.extend(orig_vals.iter().copied());
        null_block.append_operation(cf::br(&merge_block, &null_args, self.loc));

        // Call path: release recv (the actual call will re-evaluate it), then call normally.
        // We release recv here since lower_call_expression will re-evaluate receiver.
        call_block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[recv_i64], &[], self.loc,
        ));
        let mut call_scope = orig_scope.clone();
        let (result_opt, nb) = self.lower_call_expression(call, call_block, region, &mut call_scope)?;
        let call_block = nb;
        let result = result_opt.unwrap_or_else(|| {
            call_block.append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(),
                self.loc,
            )).result(0).unwrap().into()
        });
        let result_i64 = self.ensure_i64(result, call_block)?;

        let mut call_args = vec![result_i64];
        for k in &scope_keys {
            let v = *call_scope.get(k).unwrap_or(&orig_scope[k]);
            let v64 = self.ensure_i64(v, call_block).unwrap_or(v);
            call_args.push(v64);
        }
        call_block.append_operation(cf::br(&merge_block, &call_args, self.loc));

        // Update scope from merge block.
        let result_val: Value<'c, 'b> = merge_block.argument(0)?.into();
        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), merge_block.argument(i + 1)?.into());
        }

        Ok((Some(result_val), merge_block))
    }

    /// Lower a static member access, optionally guarded against null/undefined.
    fn lower_optional_static_member<'b>(
        &mut self,
        member: &oxc_ast::ast::StaticMemberExpression<'_>,
        optional: bool,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        if !optional {
            // Non-optional chain element: delegate to the regular member access in lower_expression.
            // Re-use existing logic by just doing ts_obj_get directly.
            let prop_name = member.property.name.to_string();
            let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
            block = nb;
            let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("chain member: object produced no value"))?;
            let obj_i64 = self.ensure_i64(obj, block)?;
            let key_ptr = self.get_string_ptr(&prop_name, block)?;
            let val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                &[obj_i64, key_ptr],
                &[self.i64_type()],
                self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[obj_i64], &[], self.loc,
            ));
            return Ok((Some(val), block));
        }

        // Optional: obj?.prop
        // Evaluate object.
        let prop_name = member.property.name.to_string();
        let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
        block = nb;
        let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("optional member: object produced no value"))?;
        let obj_i64 = self.ensure_i64(obj, block)?;

        // Check for null/undefined.
        let is_null: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_is_nullish"),
            &[obj_i64],
            &[self.i32_type()],
            self.loc,
        )).result(0)?.into();
        let is_null_i1 = self.ensure_i1(is_null, block)?;

        // Emit: if null → undefined, else → ts_obj_get(obj, key)
        let null_block   = region.append_block(Block::new(&[]));
        let access_block = region.append_block(Block::new(&[]));
        let merge_block  = region.append_block(Block::new(&[(self.i64_type(), self.loc)]));

        block.append_operation(cf::cond_br(
            self.ctx, is_null_i1, &null_block, &access_block, &[], &[], self.loc,
        ));

        // Null path: release obj, return undefined.
        null_block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[obj_i64], &[], self.loc,
        ));
        let undef_val: Value<'c, 'b> = null_block.append_operation(arith::constant(
            self.ctx,
            IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(),
            self.loc,
        )).result(0)?.into();
        null_block.append_operation(cf::br(&merge_block, &[undef_val], self.loc));

        // Access path: ts_obj_get(obj, key), release obj.
        let key_ptr = self.get_string_ptr(&prop_name, access_block)?;
        let result: Value<'c, 'b> = access_block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
            &[obj_i64, key_ptr],
            &[self.i64_type()],
            self.loc,
        )).result(0)?.into();
        access_block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[obj_i64], &[], self.loc,
        ));
        access_block.append_operation(cf::br(&merge_block, &[result], self.loc));

        let merged: Value<'c, 'b> = merge_block.argument(0)?.into();
        Ok((Some(merged), merge_block))
    }

    /// Lower a computed member access, optionally guarded against null/undefined.
    fn lower_optional_computed_member<'b>(
        &mut self,
        member: &oxc_ast::ast::ComputedMemberExpression<'_>,
        optional: bool,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        if !optional {
            // Non-optional: delegate to the generic computed member handler (uses ts_val_get_key).
            return self.lower_computed_member_expression(member, block, region, scope);
        }

        // Optional: obj?.[idx]
        let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
        block = nb;
        let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("optional computed: object produced no value"))?;
        let obj_i64 = self.ensure_i64(obj, block)?;

        let is_null: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_is_nullish"),
            &[obj_i64],
            &[self.i32_type()],
            self.loc,
        )).result(0)?.into();
        let is_null_i1 = self.ensure_i1(is_null, block)?;

        let null_block   = region.append_block(Block::new(&[]));
        let access_block = region.append_block(Block::new(&[]));
        let merge_block  = region.append_block(Block::new(&[(self.i64_type(), self.loc)]));

        block.append_operation(cf::cond_br(
            self.ctx, is_null_i1, &null_block, &access_block, &[], &[], self.loc,
        ));

        null_block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[obj_i64], &[], self.loc,
        ));
        let undef_val: Value<'c, 'b> = null_block.append_operation(arith::constant(
            self.ctx,
            IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(),
            self.loc,
        )).result(0)?.into();
        null_block.append_operation(cf::br(&merge_block, &[undef_val], self.loc));

        // Re-evaluate the index in the access block and use ts_val_get_key (handles strings, arrays, maps).
        let (idx_opt, nb) = self.lower_expression(&member.expression, access_block, region, scope)?;
        let access_block_after_idx = nb;
        let idx = idx_opt.ok_or_else(|| anyhow::anyhow!("optional computed: index produced no value"))?;
        let idx_i64 = self.ensure_i64(idx, access_block_after_idx)?;
        let result: Value<'c, 'b> = access_block_after_idx.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_val_get_key"),
            &[obj_i64, idx_i64],
            &[self.i64_type()],
            self.loc,
        )).result(0)?.into();
        access_block_after_idx.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[obj_i64], &[], self.loc,
        ));
        access_block_after_idx.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[idx_i64], &[], self.loc,
        ));
        access_block_after_idx.append_operation(cf::br(&merge_block, &[result], self.loc));

        let merged: Value<'c, 'b> = merge_block.argument(0)?.into();
        Ok((Some(merged), merge_block))
    }

    // ── new Foo(args) ─────────────────────────────────────────────────────

    pub(crate) fn lower_new_expression<'b>(
        &mut self,
        new_expr: &NewExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        // Handle `new X.Y(...)` — evaluate the callee dynamically and call it as a constructor.
        // e.g. `new this._Promise(executor)` where `this._Promise` is a Promise constructor function.
        if matches!(&new_expr.callee, Expression::StaticMemberExpression(_) | Expression::ComputedMemberExpression(_)) {
            let i64t = self.i64_type();
            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            // Evaluate the callee to get the constructor function value.
            let (callee_v, nb) = self.lower_expression(&new_expr.callee, block, region, scope)?;
            block = nb;
            let callee_i64 = callee_v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64);
            // Evaluate arguments.
            let mut args: Vec<Value<'c, 'b>> = Vec::new();
            for arg in &new_expr.arguments {
                if let Some(expr) = arg.as_expression() {
                    let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    args.push(v_opt.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64));
                }
            }
            let call_func = match args.len() {
                0 => "ts_func_call0",
                1 => "ts_func_call1",
                2 => "ts_func_call2",
                3 => "ts_func_call3",
                _ => "ts_func_call4",
            };
            let mut call_args = vec![callee_i64];
            call_args.extend(args.iter().take(4).copied());
            let result: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, call_func),
                &call_args, &[i64t], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[callee_i64], &[], self.loc,
            ));
            for a in &args {
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[*a], &[], self.loc,
                ));
            }
            return Ok((Some(result), block));
        }

        let Expression::Identifier(callee_id) = &new_expr.callee else {
            bail!("new: unsupported callee expression type");
        };
        // Resolve class name aliases (e.g. `class Hono` exported as `HonoBase` →
        // `new Hono(...)` inside hono-base.ts should call `__class_HonoBase_constructor`).
        let class_name = {
            let raw = callee_id.name.as_str();
            self.class_name_aliases.get(raw).cloned().unwrap_or_else(|| raw.to_string())
        };

        // new Proxy(target, handler) → transparent proxy: evaluate args, return target (retain it).
        if class_name == "Proxy" {
            let i64t = self.i64_type();
            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            let mut args: Vec<Value<'c, 'b>> = Vec::new();
            for a in &new_expr.arguments {
                if let Some(expr) = a.as_expression() {
                    let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    if let Some(v) = v_opt { args.push(self.ensure_i64(v, block)?); }
                }
            }
            let target = args.first().copied().unwrap_or(undef_i64);
            // Retain target (returned as value); release handler if present.
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"),
                &[target], &[], self.loc,
            ));
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[*a], &[], self.loc,
                    ));
                }
            }
            return Ok((Some(target), block));
        }

        // new Promise(executor) → ts_promise_new(executor)
        if class_name == "Promise" {
            let i64t = self.i64_type();
            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            let executor = if let Some(arg) = new_expr.arguments.first() {
                if let Some(expr) = arg.as_expression() {
                    let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                } else { undef_i64 }
            } else { undef_i64 };
            let promise_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_promise_new"),
                &[executor], &[i64t], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[executor], &[], self.loc,
            ));
            return Ok((Some(promise_val), block));
        }

        // Built-in constructors
        if class_name == "RegExp" {
            let i64t = self.i64_type();
            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            let mut args: Vec<Value<'c, 'b>> = Vec::new();
            for a in &new_expr.arguments {
                if let Some(expr) = a.as_expression() {
                    let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    if let Some(v) = v_opt { args.push(self.ensure_i64(v, block)?); }
                }
            }
            let src = args.first().copied().unwrap_or(undef_i64);
            let flags = args.get(1).copied().unwrap_or(undef_i64);
            let re_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_regexp_from_val"),
                &[src, flags], &[i64t], self.loc,
            )).result(0)?.into();
            for a in &args {
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[*a], &[], self.loc,
                ));
            }
            return Ok((Some(re_val), block));
        }

        // Built-in Error constructor: new Error(message?) → TsObject with 'message' property
        if class_name == "Error" || class_name == "TypeError" || class_name == "RangeError"
            || class_name == "ReferenceError" || class_name == "SyntaxError"
        {
            let i64t = self.i64_type();
            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            let msg = if let Some(arg) = new_expr.arguments.first() {
                if let Some(expr) = arg.as_expression() {
                    let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    v_opt.map(|v| -> Result<Value<'c, 'b>> { Ok(self.ensure_i64(v, block)?) })
                        .transpose()?.unwrap_or(undef_i64)
                } else { undef_i64 }
            } else { undef_i64 };
            let err_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_error_new"),
                &[msg], &[i64t], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[msg], &[], self.loc,
            ));
            return Ok((Some(err_val), block));
        }

        // Built-in Headers constructor: new Headers(init?) → TsHeaders (tag=7)
        if class_name == "Headers" {
            let i64t = self.i64_type();
            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            let init = if let Some(arg) = new_expr.arguments.first() {
                if let Some(expr) = arg.as_expression() {
                    let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    match v_opt { Some(v) => self.ensure_i64(v, block)?, None => undef_i64 }
                } else { undef_i64 }
            } else { undef_i64 };
            let headers_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_headers_new"),
                &[init], &[i64t], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[init], &[], self.loc,
            ));
            return Ok((Some(headers_val), block));
        }

        // Built-in Response constructor: new Response(body?, init?) → TsResponse (tag=8)
        if class_name == "Response" {
            let i64t = self.i64_type();
            let null_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FFC_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            let body = if let Some(arg) = new_expr.arguments.first() {
                if let Some(expr) = arg.as_expression() {
                    let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    match v_opt { Some(v) => self.ensure_i64(v, block)?, None => null_i64 }
                } else { null_i64 }
            } else { null_i64 };
            let init = if let Some(arg) = new_expr.arguments.get(1) {
                if let Some(expr) = arg.as_expression() {
                    let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    match v_opt { Some(v) => self.ensure_i64(v, block)?, None => undef_i64 }
                } else { undef_i64 }
            } else { undef_i64 };
            let resp_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_response_new"),
                &[body, init], &[i64t], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[body], &[], self.loc,
            ));
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[init], &[], self.loc,
            ));
            return Ok((Some(resp_val), block));
        }

        if class_name == "String" {
            // new String(value) — treat as String(value) coercion.
            let i64t = self.i64_type();
            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            let val = if let Some(arg) = new_expr.arguments.first() {
                if let Some(expr) = arg.as_expression() {
                    let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                } else { undef_i64 }
            } else { undef_i64 };
            let result: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_coerce_string"),
                &[val], &[i64t], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[val], &[], self.loc));
            return Ok((Some(result), block));
        }

        if class_name == "Number" {
            let i64t = self.i64_type();
            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            let val = if let Some(arg) = new_expr.arguments.first() {
                if let Some(expr) = arg.as_expression() {
                    let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                } else { undef_i64 }
            } else { undef_i64 };
            let result: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_coerce_number"),
                &[val], &[i64t], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[val], &[], self.loc));
            return Ok((Some(result), block));
        }

        if class_name == "Array" {
            // new Array(n) — create a TsArray with given capacity.
            let i64t = self.i64_type();
            let i32t = self.i32_type();
            let zero_i32: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i32t, 0).into(), self.loc,
            )).result(0)?.into();
            let cap = if let Some(arg) = new_expr.arguments.first() {
                if let Some(expr) = arg.as_expression() {
                    let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    v.map(|v| self.ensure_i32(v, block)).transpose()?.unwrap_or(zero_i32)
                } else { zero_i32 }
            } else { zero_i32 };
            let arr_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
                &[cap], &[i64t], self.loc,
            )).result(0)?.into();
            return Ok((Some(arr_val), block));
        }

        if class_name == "Request" {
            // new Request(url, init?) — create a TsObject with url/method/headers/body from init.
            let i64t = self.i64_type();
            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            let url_val = if let Some(arg) = new_expr.arguments.first() {
                if let Some(expr) = arg.as_expression() {
                    let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                } else { undef_i64 }
            } else { undef_i64 };
            let init_val = if let Some(arg) = new_expr.arguments.get(1) {
                if let Some(expr) = arg.as_expression() {
                    let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                } else { undef_i64 }
            } else { undef_i64 };
            let req_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_request_new"),
                &[url_val, init_val], &[i64t], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[url_val], &[], self.loc));
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[init_val], &[], self.loc));
            return Ok((Some(req_val), block));
        }

        if class_name == "Map" {
            let i64t = self.i64_type();
            // If initial iterable arg provided (e.g. new Map([[k,v]])), use ts_map_from_arr
            if !new_expr.arguments.is_empty() {
                if let Some(expr) = new_expr.arguments[0].as_expression() {
                    let (arr_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    if let Some(arr) = arr_opt {
                        let arr_i64 = self.ensure_i64(arr, block)?;
                        let map_val: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_map_from_arr"),
                            &[arr_i64], &[i64t], self.loc,
                        )).result(0)?.into();
                        block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                            &[arr_i64], &[], self.loc,
                        ));
                        return Ok((Some(map_val), block));
                    }
                }
            }
            let map_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_map_new"),
                &[], &[i64t], self.loc,
            )).result(0)?.into();
            return Ok((Some(map_val), block));
        }

        if class_name == "Set" {
            let i64t = self.i64_type();
            // new Set()  or  new Set(iterable)
            if new_expr.arguments.is_empty() {
                let set_val: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_set_new"),
                    &[], &[i64t], self.loc,
                )).result(0)?.into();
                return Ok((Some(set_val), block));
            } else if let Some(expr) = new_expr.arguments[0].as_expression() {
                let (iter_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                block = nb;
                let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                )).result(0)?.into();
                let iter_i64 = iter_opt
                    .map(|v| self.ensure_i64(v, block))
                    .transpose()?
                    .unwrap_or(undef_i64);
                let set_val: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_set_new_from_iter"),
                    &[iter_i64], &[i64t], self.loc,
                )).result(0)?.into();
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[iter_i64], &[], self.loc));
                return Ok((Some(set_val), block));
            }
        }

        if class_name == "WeakMap" {
            let i64t = self.i64_type();
            let wm_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_weakmap_new"),
                &[], &[i64t], self.loc,
            )).result(0)?.into();
            return Ok((Some(wm_val), block));
        }

        if class_name == "WeakSet" {
            let i64t = self.i64_type();
            let ws_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_weakset_new"),
                &[], &[i64t], self.loc,
            )).result(0)?.into();
            return Ok((Some(ws_val), block));
        }

        if class_name == "WeakRef" {
            let i64t = self.i64_type();
            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            let target_val = if let Some(arg) = new_expr.arguments.first() {
                if let Some(e) = arg.as_expression() {
                    let (v, b) = self.lower_expression(e, block, region, scope)?;
                    block = b;
                    v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                } else { undef_i64 }
            } else { undef_i64 };
            let wr_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_weakref_new"),
                &[target_val], &[i64t], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[target_val], &[], self.loc));
            return Ok((Some(wr_val), block));
        }

        if class_name == "Date" {
            let i64t = self.i64_type();
            let date_val: Value<'c, 'b> = if new_expr.arguments.is_empty() {
                // new Date() — current time
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_new"),
                    &[], &[i64t], self.loc,
                )).result(0)?.into()
            } else {
                // new Date(value) — from number or string
                let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                )).result(0)?.into();
                let arg_val = if let Some(arg) = new_expr.arguments.first() {
                    if let Some(e) = arg.as_expression() {
                        let (v, b) = self.lower_expression(e, block, region, scope)?;
                        block = b;
                        v.map(|v| self.ensure_i64(v, block).unwrap_or(v)).unwrap_or(undef_i64)
                    } else { undef_i64 }
                } else { undef_i64 };
                let result: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_from_val"),
                    &[arg_val], &[i64t], self.loc,
                )).result(0)?.into();
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[arg_val], &[], self.loc));
                result
            };
            return Ok((Some(date_val), block));
        }

        // Built-in URL constructor: new URL(href, base?) → TsObject with url properties
        if class_name == "URL" {
            let i64t = self.i64_type();
            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            let href = if let Some(arg) = new_expr.arguments.first() {
                if let Some(expr) = arg.as_expression() {
                    let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                } else { undef_i64 }
            } else { undef_i64 };
            let base = if let Some(arg) = new_expr.arguments.get(1) {
                if let Some(expr) = arg.as_expression() {
                    let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                } else { undef_i64 }
            } else { undef_i64 };
            let url_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_url_new"),
                &[href, base], &[i64t], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[href], &[], self.loc));
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[base], &[], self.loc));
            return Ok((Some(url_val), block));
        }

        // Built-in URLSearchParams constructor: new URLSearchParams(init?) → tag=9
        if class_name == "URLSearchParams" {
            let i64t = self.i64_type();
            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            let init = if let Some(arg) = new_expr.arguments.first() {
                if let Some(expr) = arg.as_expression() {
                    let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                } else { undef_i64 }
            } else { undef_i64 };
            let sp_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_urlsearchparams_new"),
                &[init], &[i64t], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[init], &[], self.loc));
            return Ok((Some(sp_val), block));
        }

        let ctor_name = format!("__class_{}_constructor", class_name);
        // If the class constructor is not statically known but the identifier is in scope
        // (e.g. `new Provider()` where Provider is a loop variable holding a TsFunction),
        // emit a dynamic call via ts_func_callN.
        if !self.funcs.contains_key(&ctor_name) {
            if let Some(&fn_val) = scope.get(&class_name) {
                let i64_type = self.i64_type();
                let mut args: Vec<Value<'c, 'b>> = Vec::new();
                for arg in &new_expr.arguments {
                    let expr = arg.as_expression()
                        .ok_or_else(|| anyhow::anyhow!("spread in dynamic new expression"))?;
                    let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    let v = v_opt.ok_or_else(|| anyhow::anyhow!("new arg produced no value"))?;
                    args.push(self.ensure_i64(v, block)?);
                }
                let fn_val_i64 = self.ensure_i64(fn_val, block)?;
                let call_func = match args.len() {
                    0 => "ts_func_call0",
                    1 => "ts_func_call1",
                    2 => "ts_func_call2",
                    3 => "ts_func_call3",
                    4 => "ts_func_call4",
                    _ => bail!("dynamic new: too many constructor args (max 4 supported)"),
                };
                let mut call_args = vec![fn_val_i64];
                call_args.extend_from_slice(&args);
                let result: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, call_func),
                    &call_args, &[i64_type], self.loc,
                )).result(0)?.into();
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[fn_val_i64], &[], self.loc,
                ));
                for a in &args {
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[*a], &[], self.loc,
                    ));
                }
                return Ok((Some(result), block));
            }
            // Class is from a skipped JS-only package — emit UNDEFINED and warn.
            tracing::warn!("new {}: unknown class (from JS-only package, returning undefined)", class_name);
            let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            return Ok((Some(undef), block));
        }
        let Some(sig) = self.funcs.get(&ctor_name).cloned() else {
            tracing::warn!("new {}: unknown class (from JS-only package, returning undefined)", class_name);
            let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            return Ok((Some(undef), block));
        };

        let i64_type = self.i64_type();
        let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
            self.ctx, IntegerAttribute::new(i64_type, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
        )).result(0)?.into();

        let mut args: Vec<Value<'c, 'b>> = Vec::new();
        for arg in &new_expr.arguments {
            let expr = arg.as_expression()
                .ok_or_else(|| anyhow::anyhow!("spread in new expression"))?;
            let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
            block = nb;
            let v = v_opt.ok_or_else(|| anyhow::anyhow!("new arg produced no value"))?;
            args.push(self.ensure_i64(v, block)?);
        }

        // Pad with undefined if fewer args than constructor params; truncate if more.
        let expected = sig.param_types.len();
        while args.len() < expected {
            args.push(undef_i64);
        }
        args.truncate(expected);

        let result_types: Vec<melior::ir::Type<'c>> =
            sig.return_type.iter().copied().collect();

        let op = block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, &ctor_name),
            &args,
            &result_types,
            self.loc,
        ));
        let result: Value<'c, 'b> = op.result(0)?.into();

        // Release args after the call — callee treats them as borrowed, caller owns them.
        // ts_release_val is a no-op for non-pointer values (UNDEFINED padding), so it's safe
        // to release all args unconditionally.
        for a in &args {
            block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[*a],
                &[],
                self.loc,
            ));
        }

        Ok((Some(result), block))
    }

}
