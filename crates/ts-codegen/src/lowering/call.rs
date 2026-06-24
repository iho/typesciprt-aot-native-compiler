use super::*;

impl<'c, 'm> Lowerer<'c, 'm> {
    // ── Call expressions ──────────────────────────────────────────────────

    pub(super) fn lower_call_expression<'b>(
        &mut self,
        call: &CallExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        // super() — direct constructor call; we pre-process these in lower_class_constructor,
        // so if we ever reach here it means super() was used in an unusual position.
        // Return 'this' from scope as a safe fallback.
        if let Expression::Super(_) = &call.callee {
            let this_val = scope.get("this")
                .copied()
                .unwrap_or_else(|| block.append_operation(arith::constant(
                    self.ctx, IntegerAttribute::new(self.i64_type(), 0).into(), self.loc,
                )).result(0).unwrap().into());
            return Ok((Some(this_val), block));
        }

        // this.#privateMethod(args) — private method call (class method or stored function)
        if let Expression::PrivateFieldExpression(priv_field) = &call.callee {
            let field_name = format!("__priv_{}", priv_field.field.name.as_str());
            // Determine class from receiver type
            let class_name_opt: Option<String> = if let Expression::ThisExpression(_) = &priv_field.object {
                self.current_class.clone()
            } else if let Expression::Identifier(id) = &priv_field.object {
                self.var_class_types.get(id.name.as_str()).cloned()
            } else {
                None
            };
            let is_class_method = class_name_opt.as_ref().map(|cn| {
                let mangled = format!("__class_{}_{}", cn, field_name);
                self.funcs.contains_key(&mangled)
            }).unwrap_or(false);

            if is_class_method {
                let class_name = class_name_opt.unwrap();
                let mangled = format!("__class_{}_{}", class_name, field_name);
                // Lower receiver
                let (obj_opt, nb) = self.lower_expression(&priv_field.object, block, region, scope)?;
                block = nb;
                let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("private method call: receiver produced no value"))?;
                let obj_i64 = self.ensure_i64(obj, block)?;
                // Lower args — handle spread by unpacking array elements
                let mut args = vec![obj_i64];
                let has_spread = call.arguments.iter().any(|a| a.as_expression().is_none());
                if has_spread {
                    // Single-spread pattern: this.#method(...arr) — unpack arr into positional args.
                    // Determine expected param count from signature (minus 1 for `this`).
                    let n_params = self.funcs.get(&mangled)
                        .map(|s| s.param_types.len().saturating_sub(1))
                        .unwrap_or(0);
                    for arg in &call.arguments {
                        if let Some(expr) = arg.as_expression() {
                            let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            let v = v_opt.ok_or_else(|| anyhow::anyhow!("private method arg produced no value"))?;
                            args.push(self.ensure_i64(v, block)?);
                        } else {
                            // Spread element: unpack from array using ts_arr_get
                            use oxc_ast::ast::Argument;
                            let Argument::SpreadElement(spread) = arg else { unreachable!() };
                            let (arr_opt, nb) = self.lower_expression(&spread.argument, block, region, scope)?;
                            block = nb;
                            let arr = arr_opt.ok_or_else(|| anyhow::anyhow!("spread array produced no value"))?.into();
                            let arr_i64 = self.ensure_i64(arr, block)?;
                            let i32t = self.i32_type();
                            let remaining = n_params.saturating_sub(args.len() - 1); // -1 for `this`
                            for idx in 0..remaining {
                                let idx_val: Value<'c, 'b> = block.append_operation(arith::constant(
                                    self.ctx,
                                    IntegerAttribute::new(i32t, idx as i64).into(),
                                    self.loc,
                                )).result(0)?.into();
                                let elem: Value<'c, 'b> = block.append_operation(func::call(
                                    self.ctx,
                                    FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
                                    &[arr_i64, idx_val],
                                    &[self.i64_type()],
                                    self.loc,
                                )).result(0)?.into();
                                args.push(elem);
                            }
                        }
                    }
                } else {
                    for arg in &call.arguments {
                        let expr = arg.as_expression().unwrap();
                        let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                        block = nb;
                        let v = v_opt.ok_or_else(|| anyhow::anyhow!("private method arg produced no value"))?;
                        args.push(self.ensure_i64(v, block)?);
                    }
                }
                let op = block.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, &mangled),
                    &args,
                    &[self.i64_type()],
                    self.loc,
                ));
                return Ok((Some(op.result(0)?.into()), block));
            } else {
                // Stored function value in private field: read it then call dynamically
                let (fn_opt, nb) = self.lower_expression(&call.callee, block, region, scope)?;
                block = nb;
                if let Some(fn_val) = fn_opt {
                    return self.lower_dynamic_call(fn_val, &call.arguments, block, region, scope);
                }
            }
        }

        // super.method(args) — dispatches to the parent class's method.
        if let Expression::StaticMemberExpression(member) = &call.callee {
            if let Expression::Super(_) = &member.object {
                let method_name = member.property.name.as_str().to_string();
                if let Some(class_name) = self.current_class.clone() {
                    if let Some(parent_name) = self.classes.get(&class_name)
                        .and_then(|s| s.parent.clone())
                    {
                        // Look up the mangled name from the parent's (inherited) method map.
                        // This handles the case where the parent itself doesn't override the
                        // method — `methods` includes inherited entries from grandparents.
                        let mangled = self.classes.get(&parent_name)
                            .and_then(|sig| sig.methods.get(&method_name))
                            .cloned()
                            .unwrap_or_else(|| format!("__class_{}_{}", parent_name, method_name));
                        let this_val = scope.get("this")
                            .copied()
                            .ok_or_else(|| anyhow::anyhow!("super.method(): no 'this' in scope"))?;
                        let this_i64 = self.ensure_i64(this_val, block)?;
                        block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"),
                            &[this_i64], &[], self.loc,
                        ));
                        let mut args = vec![this_i64];
                        for arg in &call.arguments {
                            let expr = arg.as_expression()
                                .ok_or_else(|| anyhow::anyhow!("super.method: spread not supported"))?;
                            let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            let v = v_opt.ok_or_else(|| anyhow::anyhow!("super.method arg produced no value"))?;
                            args.push(self.ensure_i64(v, block)?);
                        }
                        // Pad/truncate args to match the MLIR function signature arity.
                        if let Some(fn_sig) = self.funcs.get(&mangled).cloned() {
                            let expected = fn_sig.param_types.len();
                            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                                self.ctx,
                                IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(),
                                self.loc,
                            )).result(0)?.into();
                            while args.len() < expected { args.push(undef_i64); }
                            args.truncate(expected);
                        }
                        let op = block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, &mangled),
                            &args,
                            &[self.i64_type()],
                            self.loc,
                        ));
                        return Ok((Some(op.result(0)?.into()), block));
                    }
                }
            }
        }

        // ── Prototype-method-call patterns ────────────────────────────────────
        // Detect X.prototype.Y.call(thisArg, ...args) before generic dispatch.
        if let Expression::StaticMemberExpression(call_member) = &call.callee {
            if call_member.property.name.as_str() == "call" {
                // Helper: is the object a Y method of Object (or Object.prototype)?
                let is_obj_has_own_prop = if let Expression::StaticMemberExpression(m) = &call_member.object {
                    m.property.name.as_str() == "hasOwnProperty" && matches!(
                        &m.object,
                        Expression::Identifier(id) if id.name == "Object"
                    )
                    || m.property.name.as_str() == "hasOwnProperty" && matches!(
                        &m.object,
                        Expression::StaticMemberExpression(m2)
                            if m2.property.name.as_str() == "prototype"
                            && matches!(&m2.object, Expression::Identifier(id) if id.name == "Object")
                    )
                } else { false };

                if is_obj_has_own_prop && call.arguments.len() >= 2 {
                    // Object[.prototype].hasOwnProperty.call(obj, key) → ts_val_has_key(obj, key)
                    let obj_expr = call.arguments[0].as_expression()
                        .ok_or_else(|| anyhow::anyhow!("hasOwnProperty.call: arg0 not expression"))?;
                    let key_expr = call.arguments[1].as_expression()
                        .ok_or_else(|| anyhow::anyhow!("hasOwnProperty.call: arg1 not expression"))?;
                    let (obj_v, nb) = self.lower_expression(obj_expr, block, region, scope)?;
                    block = nb;
                    let (key_v, nb) = self.lower_expression(key_expr, block, region, scope)?;
                    block = nb;
                    let obj_i = self.ensure_i64(obj_v.unwrap_or_else(|| {
                        block.append_operation(arith::constant(self.ctx,
                            IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                        )).result(0).unwrap().into()
                    }), block)?;
                    let key_i = self.ensure_i64(key_v.unwrap_or_else(|| {
                        block.append_operation(arith::constant(self.ctx,
                            IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                        )).result(0).unwrap().into()
                    }), block)?;
                    let result: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_has_key"),
                        &[obj_i, key_i], &[self.i64_type()], self.loc,
                    )).result(0)?.into();
                    return Ok((Some(result), block));
                }

                // Function.prototype.toString.call(fn) → ts_coerce_string(fn)
                let is_fn_to_string = if let Expression::StaticMemberExpression(m) = &call_member.object {
                    m.property.name.as_str() == "toString" && matches!(
                        &m.object,
                        Expression::StaticMemberExpression(m2)
                            if m2.property.name.as_str() == "prototype"
                            && matches!(&m2.object, Expression::Identifier(id) if id.name == "Function")
                    )
                } else { false };

                if is_fn_to_string {
                    // Function.prototype.toString.call(fn) → coerce fn to string
                    let this_expr = call.arguments.first()
                        .and_then(|a| a.as_expression());
                    let fn_i = if let Some(expr) = this_expr {
                        let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                        block = nb;
                        self.ensure_i64(v.unwrap_or_else(|| {
                            block.append_operation(arith::constant(self.ctx,
                                IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                            )).result(0).unwrap().into()
                        }), block)?
                    } else {
                        block.append_operation(arith::constant(self.ctx,
                            IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                        )).result(0)?.into()
                    };
                    let result: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_coerce_string"),
                        &[fn_i], &[self.i64_type()], self.loc,
                    )).result(0)?.into();
                    return Ok((Some(result), block));
                }
            }
        }

        // console.log/error/warn/info/debug(a, b, …) → print each arg space-separated, then newline
        if let Expression::StaticMemberExpression(member) = &call.callee {
            if matches!(&member.object, Expression::Identifier(id) if id.name == "console")
                && matches!(member.property.name.as_str(), "log" | "error" | "warn" | "info" | "debug")
            {
                let nargs = call.arguments.len();
                for (i, arg) in call.arguments.iter().enumerate() {
                    let expr = arg.as_expression()
                        .ok_or_else(|| anyhow::anyhow!("console.log: spread argument not supported"))?;
                    let (val_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    let val = val_opt
                        .ok_or_else(|| anyhow::anyhow!("console.log: argument produced no value"))?;
                    let val_i64 = self.ensure_i64(val, block)?;
                    let is_last = i + 1 == nargs;
                    if is_last {
                        block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "__ts_console_log_val"),
                            &[val_i64], &[], self.loc,
                        ));
                    } else {
                        block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "__ts_console_log_val_inline"),
                            &[val_i64], &[], self.loc,
                        ));
                        block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "__ts_console_log_space"),
                            &[], &[], self.loc,
                        ));
                    }
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[val_i64], &[], self.loc,
                    ));
                }
                if nargs == 0 {
                    // console.log() → just a newline
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "__ts_console_log_newline"),
                        &[], &[], self.loc,
                    ));
                }
                return Ok((None, block));
            }
        }

        // Built-in: process.exit(code?)
        if let Expression::StaticMemberExpression(member) = &call.callee {
            if matches!(&member.object, Expression::Identifier(id) if id.name == "process")
                && member.property.name.as_str() == "exit"
            {
                let i64t = self.i64_type();
                let code_val = if let Some(arg) = call.arguments.first() {
                    if let Some(expr) = arg.as_expression() {
                        let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                        block = nb;
                        let vi64 = v.map(|v| self.ensure_i64(v, block)).transpose()?
                            .unwrap_or_else(|| block.append_operation(arith::constant(
                                self.ctx, IntegerAttribute::new(i64t, 0x7FFE_0000_0000_0000u64 as i64).into(), self.loc,
                            )).result(0).unwrap().into());
                        self.ensure_i32(vi64, block)?
                    } else {
                        self.lower_numeric_literal(0, block)?
                    }
                } else {
                    self.lower_numeric_literal(0, block)?
                };
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_process_exit"),
                    &[code_val], &[], self.loc,
                ));
                return Ok((None, block));
            }
        }

        // Built-in: process.cwd() / process.hrtime() / process.uptime()
        if let Expression::StaticMemberExpression(member) = &call.callee {
            if matches!(&member.object, Expression::Identifier(id) if id.name == "process") {
                let rt_name = match member.property.name.as_str() {
                    "cwd"    => Some("ts_process_cwd"),
                    "hrtime" => Some("ts_process_hrtime"),
                    "uptime" => Some("ts_process_uptime"),
                    _ => None,
                };
                if let Some(rt) = rt_name {
                    let val: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, rt),
                        &[], &[self.i64_type()], self.loc,
                    )).result(0)?.into();
                    return Ok((Some(val), block));
                }
            }
        }

        // Built-in: performance.now() / performance.mark(name) / performance.measure(name, start)
        if let Expression::StaticMemberExpression(member) = &call.callee {
            if matches!(&member.object, Expression::Identifier(id) if id.name == "performance") {
                match member.property.name.as_str() {
                    "now" => {
                        let val: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_performance_now"),
                            &[], &[self.i64_type()], self.loc,
                        )).result(0)?.into();
                        return Ok((Some(val), block));
                    }
                    "mark" => {
                        let i64t = self.i64_type();
                        let undef: Value<'c, '_> = block.append_operation(arith::constant(
                            self.ctx,
                            IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(),
                            self.loc,
                        )).result(0).unwrap().into();
                        let name_arg = if let Some(arg) = call.arguments.first() {
                            if let Some(expr) = arg.as_expression() {
                                let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                                block = nb;
                                v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef)
                            } else { undef }
                        } else { undef };
                        let val: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_performance_mark"),
                            &[name_arg], &[i64t], self.loc,
                        )).result(0)?.into();
                        return Ok((Some(val), block));
                    }
                    _ => {}
                }
            }
        }

        // Built-in: addEventListener(event, handler) — registers a global fetch handler
        if let Expression::Identifier(callee_id) = &call.callee {
            if callee_id.name == "addEventListener" {
                let i64t = self.i64_type();
                let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                )).result(0)?.into();
                let event = if let Some(arg) = call.arguments.first() {
                    if let Some(expr) = arg.as_expression() {
                        let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                        block = nb;
                        v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                    } else { undef_i64 }
                } else { undef_i64 };
                let handler = if let Some(arg) = call.arguments.get(1) {
                    if let Some(expr) = arg.as_expression() {
                        let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                        block = nb;
                        v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                    } else { undef_i64 }
                } else { undef_i64 };
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_add_event_listener"),
                    &[event, handler], &[i64t], self.loc,
                ));
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[event], &[], self.loc));
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[handler], &[], self.loc));
                return Ok((None, block));
            }
            if callee_id.name == "removeEventListener" {
                let i64t = self.i64_type();
                let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                )).result(0)?.into();
                let event = if let Some(arg) = call.arguments.first() {
                    if let Some(expr) = arg.as_expression() {
                        let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                        block = nb;
                        v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                    } else { undef_i64 }
                } else { undef_i64 };
                let handler = if let Some(arg) = call.arguments.get(1) {
                    if let Some(expr) = arg.as_expression() {
                        let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                        block = nb;
                        v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                    } else { undef_i64 }
                } else { undef_i64 };
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_remove_event_listener"),
                    &[event, handler], &[i64t], self.loc,
                ));
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[event], &[], self.loc));
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[handler], &[], self.loc));
                return Ok((None, block));
            }
        }

        // TypeScript compiler helper functions emitted by tsc.
        // __decorate(decorators, target, key?, desc?) — apply an array of decorators.
        // __metadata(key, value) — returns a decorator that calls Reflect.defineMetadata.
        // __extends(child, parent) — sets up prototype chain (class inheritance).
        // __spreadArray(to, from, pack) — spreads elements from `from` into `to`.
        // __assign(target, ...sources) — Object.assign polyfill.
        // __awaiter / __generator — handled by our native async/generator support; return UNDEFINED.
        if let Expression::Identifier(callee_id) = &call.callee {
            let i64t = self.i64_type();
            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            match callee_id.name.as_str() {
                "__decorate" => {
                    // __decorate([dec1, dec2, ...], target, key, desc)
                    // Evaluate each decorator and apply it. Mirrors TypeScript's __decorate helper.
                    // We call ts_apply_decorators(decorators_array, target, key, desc).
                    let mut arg_vals: Vec<Value<'c, 'b>> = Vec::new();
                    for arg in &call.arguments {
                        if let Some(expr) = arg.as_expression() {
                            let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            let v_i64 = v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64);
                            arg_vals.push(v_i64);
                        } else {
                            arg_vals.push(undef_i64);
                        }
                    }
                    while arg_vals.len() < 4 { arg_vals.push(undef_i64); }
                    let result: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_apply_decorators"),
                        &arg_vals[..4], &[i64t], self.loc,
                    )).result(0)?.into();
                    for v in &arg_vals { block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[*v], &[], self.loc)); }
                    return Ok((Some(result), block));
                }
                "__metadata" => {
                    // __metadata(key, value) → returns a decorator function that calls
                    // Reflect.defineMetadata(key, value, target, propertyKey).
                    // We emit ts_reflect_metadata_decorator(key, value) which returns a TsFunction.
                    let key_val = if let Some(arg) = call.arguments.first() {
                        if let Some(expr) = arg.as_expression() {
                            let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                        } else { undef_i64 }
                    } else { undef_i64 };
                    let val_val = if let Some(arg) = call.arguments.get(1) {
                        if let Some(expr) = arg.as_expression() {
                            let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                        } else { undef_i64 }
                    } else { undef_i64 };
                    let decorator: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_reflect_metadata_decorator"),
                        &[key_val, val_val], &[i64t], self.loc,
                    )).result(0)?.into();
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[key_val], &[], self.loc));
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[val_val], &[], self.loc));
                    return Ok((Some(decorator), block));
                }
                "__extends" => {
                    // __extends(ChildClass, ParentClass) — set up prototype chain.
                    // In our NaN-boxed system, inheritance is handled at class init time.
                    // For JS-compiled TypeScript, this is a no-op since our class lowering
                    // already established inheritance. Return undefined.
                    for arg in &call.arguments {
                        if let Some(expr) = arg.as_expression() {
                            let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            if let Some(v) = v {
                                let v_i64 = self.ensure_i64(v, block)?;
                                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[v_i64], &[], self.loc));
                            }
                        }
                    }
                    return Ok((Some(undef_i64), block));
                }
                "__spreadArray" => {
                    // __spreadArray(to, from, pack) → ts_arr_concat(to, from)
                    let to_val = if let Some(arg) = call.arguments.first() {
                        if let Some(expr) = arg.as_expression() {
                            let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                        } else { undef_i64 }
                    } else { undef_i64 };
                    let from_val = if let Some(arg) = call.arguments.get(1) {
                        if let Some(expr) = arg.as_expression() {
                            let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                        } else { undef_i64 }
                    } else { undef_i64 };
                    // consume optional third arg
                    if let Some(arg) = call.arguments.get(2) {
                        if let Some(expr) = arg.as_expression() {
                            let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            if let Some(v) = v { let v64 = self.ensure_i64(v, block)?; block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[v64], &[], self.loc)); }
                        }
                    }
                    let result: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_concat"),
                        &[to_val, from_val], &[i64t], self.loc,
                    )).result(0)?.into();
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[to_val], &[], self.loc));
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[from_val], &[], self.loc));
                    return Ok((Some(result), block));
                }
                "__assign" => {
                    // __assign(target, ...sources) → ts_obj_assign(target, source) for each
                    let target_val = if let Some(arg) = call.arguments.first() {
                        if let Some(expr) = arg.as_expression() {
                            let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                        } else { undef_i64 }
                    } else { undef_i64 };
                    for arg in call.arguments.iter().skip(1) {
                        if let Some(expr) = arg.as_expression() {
                            let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            if let Some(v) = v {
                                let v64 = self.ensure_i64(v, block)?;
                                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_assign"), &[target_val, v64], &[], self.loc));
                                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[v64], &[], self.loc));
                            }
                        }
                    }
                    return Ok((Some(target_val), block));
                }
                "__awaiter" | "__generator" | "__asyncGenerator" | "__asyncDelegator" | "__asyncValues" => {
                    // Our compiler handles async/generators natively; these tsc helpers are no-ops.
                    for arg in &call.arguments {
                        if let Some(expr) = arg.as_expression() {
                            let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            if let Some(v) = v { let v64 = self.ensure_i64(v, block)?; block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[v64], &[], self.loc)); }
                        }
                    }
                    return Ok((Some(undef_i64), block));
                }
                "__classPrivateFieldGet" => {
                    // __classPrivateFieldGet(receiver, state, kind) or (receiver, privateMap, "f")
                    // Simplify: just return UNDEFINED. Private field access is handled at class level.
                    for arg in &call.arguments {
                        if let Some(expr) = arg.as_expression() {
                            let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            if let Some(v) = v { let v64 = self.ensure_i64(v, block)?; block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[v64], &[], self.loc)); }
                        }
                    }
                    return Ok((Some(undef_i64), block));
                }
                "__classPrivateFieldSet" => {
                    // __classPrivateFieldSet(receiver, state, value, kind) — return value arg
                    let value_val = if let Some(arg) = call.arguments.get(2) {
                        if let Some(expr) = arg.as_expression() {
                            let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                        } else { undef_i64 }
                    } else { undef_i64 };
                    // release other args
                    for (i, arg) in call.arguments.iter().enumerate() {
                        if i == 2 { continue; }
                        if let Some(expr) = arg.as_expression() {
                            let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            if let Some(v) = v { let v64 = self.ensure_i64(v, block)?; block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[v64], &[], self.loc)); }
                        }
                    }
                    return Ok((Some(value_val), block));
                }
                _ => {}
            }
        }

        // require('specifier') — CommonJS interop.
        // When the result is destructured at the call site (`const { a, b } = require('x')`),
        // the variables are already resolved as module globals from the loaded CJS module.
        // For `const X = require('x')` we call ts_cjs_require_ns to return the namespace object.
        if let Expression::Identifier(callee_id) = &call.callee {
            if callee_id.name == "require" {
                if let Some(arg) = call.arguments.first() {
                    if let Some(Expression::StringLiteral(spec)) = arg.as_expression() {
                        let spec_str = spec.value.to_string();
                        // Return the CJS namespace object for this module
                        let spec_val = self.lower_string_literal(&spec_str, block)?;
                        let ns: melior::ir::Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_cjs_require_ns"),
                            &[spec_val],
                            &[self.i64_type()],
                            self.loc,
                        )).result(0)?.into();
                        // ts_cjs_require_ns takes ownership of spec_val (retains internally, releases arg)
                        return Ok((Some(ns), block));
                    }
                }
                // require() with non-literal arg — return UNDEFINED
                let undef: melior::ir::Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx,
                    IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(),
                    self.loc,
                )).result(0)?.into();
                return Ok((Some(undef), block));
            }
        }

        // Built-in: serve(port) with registered listener → ts_serve_worker(port: i32)
        if let Expression::Identifier(callee_id) = &call.callee {
            if callee_id.name == "serve" && call.arguments.len() == 1 {
                let port_expr = call.arguments[0].as_expression()
                    .ok_or_else(|| anyhow::anyhow!("serve: spread not supported"))?;
                let (port_opt, nb) = self.lower_expression(port_expr, block, region, scope)?;
                block = nb;
                let port_val = port_opt.ok_or_else(|| anyhow::anyhow!("serve: port produced no value"))?;
                let port_i32 = self.ensure_i32(port_val, block)?;
                block.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_serve_worker"),
                    &[port_i32],
                    &[self.i64_type()],
                    self.loc,
                ));
                return Ok((None, block));
            }
        }

        // Built-in: serve(port, fetchFn) → ts_serve(port: i32, fetch_fn: i64) — blocks forever
        if let Expression::Identifier(callee_id) = &call.callee {
            if callee_id.name == "serve" && call.arguments.len() == 2 {
                let port_expr = call.arguments[0].as_expression()
                    .ok_or_else(|| anyhow::anyhow!("serve: spread not supported"))?;
                let fn_expr = call.arguments[1].as_expression()
                    .ok_or_else(|| anyhow::anyhow!("serve: spread not supported"))?;
                let (port_opt, nb) = self.lower_expression(port_expr, block, region, scope)?;
                block = nb;
                let port_val = port_opt.ok_or_else(|| anyhow::anyhow!("serve: port produced no value"))?;
                let port_i32 = self.ensure_i32(port_val, block)?;
                let (fn_opt, nb) = self.lower_expression(fn_expr, block, region, scope)?;
                block = nb;
                let fetch_fn = fn_opt.ok_or_else(|| anyhow::anyhow!("serve: handler produced no value"))?;
                let fetch_i64 = self.ensure_i64(fetch_fn, block)?;
                block.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_serve"),
                    &[port_i32, fetch_i64],
                    &[self.i64_type()],
                    self.loc,
                ));
                return Ok((None, block));
            }
        }

        // Built-in: fetch(url, init?) → ts_fetch(url: i64, init: i64) → Promise<Response>
        if let Expression::Identifier(callee_id) = &call.callee {
            if callee_id.name == "fetch" && !self.funcs.contains_key("fetch") {
                let i64t = self.i64_type();
                let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                )).result(0)?.into();
                let url_val = if let Some(arg) = call.arguments.first() {
                    if let Some(expr) = arg.as_expression() {
                        let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                        block = nb;
                        v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                    } else { undef_i64 }
                } else { undef_i64 };
                let init_val = if let Some(arg) = call.arguments.get(1) {
                    if let Some(expr) = arg.as_expression() {
                        let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                        block = nb;
                        v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                    } else { undef_i64 }
                } else { undef_i64 };
                let promise: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_fetch"),
                    &[url_val, init_val], &[i64t], self.loc,
                )).result(0)?.into();
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[url_val], &[], self.loc));
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[init_val], &[], self.loc));
                return Ok((Some(promise), block));
            }
        }

        // Built-in: sleep(ms) → ts_sleep(ms: i32) → i64 (Promise)
        if let Expression::Identifier(callee_id) = &call.callee {
            if callee_id.name == "sleep" {
                if let Some(first_arg) = call.arguments.first() {
                    let expr = first_arg.as_expression()
                        .ok_or_else(|| anyhow::anyhow!("sleep: spread argument not supported"))?;
                    let (val_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    let val = val_opt.ok_or_else(|| anyhow::anyhow!("sleep: argument produced no value"))?;
                    let ms = self.ensure_i32(val, block)?;
                    let promise: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_sleep"),
                        &[ms],
                        &[self.i64_type()],
                        self.loc,
                    )).result(0)?.into();
                    return Ok((Some(promise), block));
                }
            }
        }

        // Built-in: setTimeout(callback, ms) / setInterval(callback, ms) / clearTimeout(id) / clearInterval(id)
        if let Expression::Identifier(callee_id) = &call.callee {
            let callee_name = callee_id.name.as_str();
            if matches!(callee_name, "setTimeout" | "setInterval" | "clearTimeout" | "clearInterval") {
                if !self.funcs.contains_key(callee_name) {
                    let i64t = self.i64_type();
                    let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                        self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                    )).result(0)?.into();
                    let mut arg_vals: Vec<Value<'c, 'b>> = Vec::new();
                    for arg in &call.arguments {
                        if let Some(expr) = arg.as_expression() {
                            let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            let v = v_opt.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64);
                            arg_vals.push(v);
                        }
                    }
                    let rt_name = match callee_name {
                        "setTimeout"   => "ts_set_timeout",
                        "setInterval"  => "ts_set_interval",
                        "clearTimeout" => "ts_clear_timeout",
                        _              => "ts_clear_interval",
                    };
                    // Pad to expected arity: set* takes 2 args, clear* takes 1.
                    let arity = if callee_name.starts_with("set") { 2 } else { 1 };
                    while arg_vals.len() < arity { arg_vals.push(undef_i64); }
                    let result: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, rt_name),
                        &arg_vals[..arity], &[i64t], self.loc,
                    )).result(0)?.into();
                    for av in &arg_vals {
                        block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                            &[*av], &[], self.loc,
                        ));
                    }
                    return Ok((Some(result), block));
                }
            }
        }

        // Built-in: structuredClone(val) — deep clone
        if let Expression::Identifier(callee_id) = &call.callee {
            if callee_id.name == "structuredClone" && !self.funcs.contains_key("structuredClone") {
                let i64t = self.i64_type();
                let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                )).result(0)?.into();
                let arg_val = if let Some(arg) = call.arguments.first() {
                    if let Some(expr) = arg.as_expression() {
                        let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                        block = nb;
                        v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                    } else { undef_i64 }
                } else { undef_i64 };
                let result: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_structured_clone"),
                    &[arg_val], &[i64t], self.loc,
                )).result(0)?.into();
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[arg_val], &[], self.loc));
                return Ok((Some(result), block));
            }
        }

        // Built-in: queueMicrotask(callback)
        if let Expression::Identifier(callee_id) = &call.callee {
            if callee_id.name == "queueMicrotask" && !self.funcs.contains_key("queueMicrotask") {
                let i64t = self.i64_type();
                let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                )).result(0)?.into();
                let cb_val = if let Some(arg) = call.arguments.first() {
                    if let Some(expr) = arg.as_expression() {
                        let (v, nb) = self.lower_expression(expr, block, region, scope)?;
                        block = nb;
                        v.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                    } else { undef_i64 }
                } else { undef_i64 };
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_queue_microtask"),
                    &[cb_val], &[i64t], self.loc,
                ));
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[cb_val], &[], self.loc));
                return Ok((None, block));
            }
        }

        // Built-in: select(p1, p2) → ts_promise_race(p1, p2) → i64 (Promise)
        if let Expression::Identifier(callee_id) = &call.callee {
            if callee_id.name == "select" && call.arguments.len() == 2 {
                let expr1 = call.arguments[0].as_expression()
                    .ok_or_else(|| anyhow::anyhow!("select: spread not supported"))?;
                let expr2 = call.arguments[1].as_expression()
                    .ok_or_else(|| anyhow::anyhow!("select: spread not supported"))?;
                let (v1_opt, nb) = self.lower_expression(expr1, block, region, scope)?;
                block = nb;
                let v1 = v1_opt.ok_or_else(|| anyhow::anyhow!("select arg1 produced no value"))?;
                let (v2_opt, nb) = self.lower_expression(expr2, block, region, scope)?;
                block = nb;
                let v2 = v2_opt.ok_or_else(|| anyhow::anyhow!("select arg2 produced no value"))?;
                let p1 = self.ensure_i64(v1, block)?;
                let p2 = self.ensure_i64(v2, block)?;
                let promise: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_promise_race"),
                    &[p1, p2],
                    &[self.i64_type()],
                    self.loc,
                )).result(0)?.into();
                return Ok((Some(promise), block));
            }
        }

        // ── Builtin alias resolution: `const foo = bar` — redirect foo(...) to bar(...) ──
        if let Expression::Identifier(callee_id) = &call.callee {
            if let Some(canonical) = self.builtin_aliases.get(callee_id.name.as_str()).cloned() {
                if !self.funcs.contains_key(&canonical) {
                    // Build a synthetic call expression with the canonical name by rebinding the callee.
                    // Simplest: re-enter lower_call_expression with argument list unchanged.
                    let mut arg_vals: Vec<Value<'c, 'b>> = Vec::new();
                    let i64t = self.i64_type();
                    let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                        self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                    )).result(0)?.into();
                    for arg in &call.arguments {
                        if let Some(expr) = arg.as_expression() {
                            let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            if let Some(v) = v_opt { arg_vals.push(self.ensure_i64(v, block)?); }
                        }
                    }
                    // Call the canonical built-in.
                    let result = self.call_builtin_by_name(&canonical, &arg_vals, undef_i64, block)?;
                    for av in &arg_vals {
                        block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                            &[*av], &[], self.loc,
                        ));
                    }
                    return Ok((result, block));
                }
            }
        }

        // ── Global built-in functions: parseInt, parseFloat, isNaN, isFinite, Number, String ──
        if let Expression::Identifier(callee_id) = &call.callee {
            let callee_name = callee_id.name.as_str();
            match callee_name {
                "parseInt" | "parseFloat" | "isNaN" | "isFinite" | "Number" | "String" | "Boolean"
                | "Error" | "TypeError" | "RangeError" | "SyntaxError" | "ReferenceError"
                | "encodeURIComponent" | "decodeURIComponent" | "encodeURI" | "decodeURI"
                | "Symbol" => {
                    if !self.funcs.contains_key(callee_name) {
                        let i64t = self.i64_type();
                        let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                            self.ctx,
                            IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(),
                            self.loc,
                        )).result(0)?.into();
                        let mut arg_vals: Vec<Value<'c, 'b>> = Vec::new();
                        for arg in &call.arguments {
                            if let Some(expr) = arg.as_expression() {
                                let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                                block = nb;
                                if let Some(v) = v_opt {
                                    arg_vals.push(self.ensure_i64(v, block)?);
                                }
                            }
                        }
                        let result: Value<'c, 'b> = match callee_name {
                            "parseInt" => {
                                let s = arg_vals.first().copied().unwrap_or(undef_i64);
                                let r = arg_vals.get(1).copied().unwrap_or(undef_i64);
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_parse_int"),
                                    &[s, r], &[i64t], self.loc,
                                )).result(0)?.into()
                            }
                            "parseFloat" => {
                                let s = arg_vals.first().copied().unwrap_or(undef_i64);
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_parse_float"),
                                    &[s], &[i64t], self.loc,
                                )).result(0)?.into()
                            }
                            "isNaN" => {
                                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_nan_val"),
                                    &[v], &[i64t], self.loc,
                                )).result(0)?.into()
                            }
                            "isFinite" => {
                                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_finite_val"),
                                    &[v], &[i64t], self.loc,
                                )).result(0)?.into()
                            }
                            "Number" => {
                                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_coerce_number"),
                                    &[v], &[i64t], self.loc,
                                )).result(0)?.into()
                            }
                            "String" => {
                                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_coerce_string"),
                                    &[v], &[i64t], self.loc,
                                )).result(0)?.into()
                            }
                            "Boolean" => {
                                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_coerce_bool"),
                                    &[v], &[i64t], self.loc,
                                )).result(0)?.into()
                            }
                            "Error" | "TypeError" | "RangeError" | "SyntaxError" | "ReferenceError" => {
                                // Error(msg) as a function call — create an error object
                                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_error_new"),
                                    &[v], &[i64t], self.loc,
                                )).result(0)?.into()
                            }
                            "encodeURIComponent" => {
                                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_encode_uri_component"),
                                    &[v], &[i64t], self.loc,
                                )).result(0)?.into()
                            }
                            "decodeURIComponent" => {
                                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_decode_uri_component"),
                                    &[v], &[i64t], self.loc,
                                )).result(0)?.into()
                            }
                            "encodeURI" => {
                                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_encode_uri"),
                                    &[v], &[i64t], self.loc,
                                )).result(0)?.into()
                            }
                            "decodeURI" => {
                                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_decode_uri"),
                                    &[v], &[i64t], self.loc,
                                )).result(0)?.into()
                            }
                            "Symbol" => {
                                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_symbol_new"),
                                    &[v], &[i64t], self.loc,
                                )).result(0)?.into()
                            }
                            _ => unreachable!(),
                        };
                        for av in &arg_vals {
                            block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                &[*av], &[], self.loc,
                            ));
                        }
                        return Ok((Some(result), block));
                    }
                }
                _ => {}
            }
        }

        // Dynamic function call: callee is an identifier holding a TsFunction (i64) in scope.
        // Local scope variables always take precedence over top-level function names.
        if let Expression::Identifier(callee_id) = &call.callee {
            let name = callee_id.name.as_str();
            let in_scope_as_i64 = scope.get(name)
                .map(|v| v.r#type() == self.i64_type())
                .unwrap_or(false);
            if in_scope_as_i64 {
                let raw = scope[name];
                // If the callee is a cell var, read the actual function value through the cell.
                let fn_val = if self.is_cell_var(name) {
                    self.cell_read(raw, block)?
                } else {
                    raw
                };
                return self.lower_dynamic_call(fn_val, &call.arguments, block, region, scope);
            }
        }

        // User-defined function call
        if let Expression::Identifier(callee_id) = &call.callee {
            let raw_name = callee_id.name.to_string();
            // Resolve alias: `const foo = bar` or `import { parse as pathParse }` — use the canonical name for the MLIR call.
            let name = if let Some(canon) = self.builtin_aliases.get(&raw_name).or_else(|| self.module_global_aliases.get(&raw_name)) {
                if self.funcs.contains_key(canon.as_str()) { canon.clone() } else { raw_name }
            } else { raw_name };
            if let Some(sig) = self.funcs.get(&name).cloned() {
                // Number of regular (non-rest) params.
                let n_regular = if sig.has_rest {
                    sig.param_types.len().saturating_sub(1)
                } else {
                    sig.param_types.len()
                };

                // Lower all call arguments (handling SpreadElement by flattening the array).
                let mut all_arg_vals: Vec<Value<'c, 'b>> = Vec::new();
                // When a spread element maps directly to the rest parameter, capture it here
                // instead of unrolling (e.g. `sum(...nums)` where sum has a rest param).
                let mut rest_direct: Option<Value<'c, 'b>> = None;
                let i32t = self.i32_type();
                let i64t2 = self.i64_type();
                for arg in call.arguments.iter() {
                    match arg {
                        oxc_ast::ast::Argument::SpreadElement(spread) => {
                            // Evaluate spread expression (should be TsArray).
                            let (v_opt, nb) = self.lower_expression(&spread.argument, block, region, scope)?;
                            block = nb;
                            let arr = v_opt.ok_or_else(|| anyhow::anyhow!("spread arg produced no value"))?;
                            let arr_i64 = self.ensure_i64(arr, block)?;

                            // If we're at/past n_regular and the function has a rest param,
                            // pass this array directly as the rest argument.
                            if sig.has_rest && all_arg_vals.len() >= n_regular {
                                rest_direct = Some(arr_i64);
                                // Don't release — ownership transferred to rest_direct
                            } else {
                                // Unroll into regular param positions.
                                let len_val: Value<'c, 'b> = block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_length"),
                                    &[arr_i64], &[i64t2], self.loc,
                                )).result(0)?.into();
                                let len_i32 = self.ensure_i32(len_val, block)?;
                                let needed = n_regular.saturating_sub(all_arg_vals.len()).min(8);
                                for idx in 0..needed {
                                    let idx_c: Value<'c, 'b> = block.append_operation(arith::constant(
                                        self.ctx, IntegerAttribute::new(i32t, idx as i64).into(), self.loc,
                                    )).result(0)?.into();
                                    let in_bounds: Value<'c, 'b> = block.append_operation(arith::cmpi(
                                        self.ctx, arith::CmpiPredicate::Slt, idx_c, len_i32, self.loc,
                                    )).result(0)?.into();
                                    let elem: Value<'c, 'b> = block.append_operation(func::call(
                                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
                                        &[arr_i64, idx_c], &[i64t2], self.loc,
                                    )).result(0)?.into();
                                    let undef_c: Value<'c, 'b> = block.append_operation(arith::constant(
                                        self.ctx, IntegerAttribute::new(i64t2, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                                    )).result(0)?.into();
                                    let selected: Value<'c, 'b> = block.append_operation(
                                        OperationBuilder::new("arith.select", self.loc)
                                            .add_operands(&[in_bounds, elem, undef_c])
                                            .add_results(&[i64t2])
                                            .build()?
                                    ).result(0)?.into();
                                    all_arg_vals.push(selected);
                                }
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                    &[arr_i64], &[], self.loc,
                                ));
                            }
                        }
                        _ => {
                            let expr = arg.as_expression()
                                .ok_or_else(|| anyhow::anyhow!("argument produced no value"))?;
                            let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            let v = v_opt.ok_or_else(|| anyhow::anyhow!("argument produced no value"))?;
                            all_arg_vals.push(self.ensure_i64(v, block)?);
                        }
                    }
                }

                let mut args: Vec<Value<'c, 'b>> = Vec::new();
                // Regular params: pass args 0..n_regular, pad with undefined if needed.
                for i in 0..n_regular {
                    if let Some(&v) = all_arg_vals.get(i) {
                        args.push(v);
                    } else {
                        let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                            self.ctx,
                            IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(),
                            self.loc,
                        )).result(0)?.into();
                        args.push(undef);
                    }
                }
                // Rest param: bundle excess args into a TsArray.
                if sig.has_rest {
                    if let Some(direct) = rest_direct {
                        // A spread element was passed directly as the rest arg — use it as-is.
                        args.push(direct);
                    } else {
                        let rest_args = if all_arg_vals.len() > n_regular {
                            &all_arg_vals[n_regular..]
                        } else {
                            &[]
                        };
                        let n_rest = rest_args.len() as i32;
                        let i32_type = self.i32_type();
                        let n_val: Value<'c, 'b> = block.append_operation(arith::constant(
                            self.ctx,
                            IntegerAttribute::new(i32_type, n_rest as i64).into(),
                            self.loc,
                        )).result(0)?.into();
                        let rest_arr: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
                            &[n_val],
                            &[self.i64_type()],
                            self.loc,
                        )).result(0)?.into();
                        for (idx, &rv) in rest_args.iter().enumerate() {
                            let idx_val: Value<'c, 'b> = block.append_operation(arith::constant(
                                self.ctx,
                                IntegerAttribute::new(i32_type, idx as i64).into(),
                                self.loc,
                            )).result(0)?.into();
                            block.append_operation(func::call(
                                self.ctx,
                                FlatSymbolRefAttribute::new(self.ctx, "ts_arr_set"),
                                &[rest_arr, idx_val, rv],
                                &[],
                                self.loc,
                            ));
                        }
                        args.push(rest_arr);
                    }
                }

                let result_types: Vec<melior::ir::Type<'c>> =
                    sig.return_type.iter().copied().collect();

                let op = block.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, &name),
                    &args,
                    &result_types,
                    self.loc,
                ));

                return if sig.return_type.is_some() {
                    Ok((Some(op.result(0)?.into()), block))
                } else {
                    Ok((None, block))
                };
            }
        }

        // ── Math.* / Object.* / Array.isArray() ──────────────────────────────
        if let Expression::StaticMemberExpression(member) = &call.callee {
            if let Expression::Identifier(ns_id) = &member.object {
                let ns = ns_id.name.as_str();
                let method = member.property.name.as_str().to_string();

                // Evaluate all arguments
                let mut arg_vals: Vec<Value<'c, 'b>> = Vec::new();
                let mut need_args = true;
                if ns == "Math" || ns == "Object" || ns == "Array" || ns == "String" || ns == "JSON" || ns == "Promise" || ns == "Reflect" || ns == "Date" || ns == "Number" {
                    for arg in &call.arguments {
                        if let Some(expr) = arg.as_expression() {
                            let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            if let Some(v) = v_opt {
                                arg_vals.push(self.ensure_i64(v, block)?);
                            }
                        }
                    }
                    need_args = false;
                }

                if !need_args {
                    let i64t = self.i64_type();
                    let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                        self.ctx,
                        IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(),
                        self.loc,
                    )).result(0)?.into();

                    let result_opt: Option<Value<'c, 'b>> = match (ns, method.as_str()) {
                        // ── Math ──────────────────────────────────────────────
                        ("Math", "abs")    => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_abs"),    &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "floor")  => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_floor"),  &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "ceil")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_ceil"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "round")  => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_round"),  &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "sqrt")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_sqrt"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "trunc")  => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_trunc"),  &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "log")    => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_log"),    &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "log2")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_log2"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "log10")  => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_log10"),  &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "sin")    => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_sin"),    &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "cos")    => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_cos"),    &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "tan")    => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_tan"),    &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "sign")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_sign"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "asin")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_asin"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "acos")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_acos"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "atan")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_atan"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "sinh")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_sinh"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "cosh")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_cosh"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "tanh")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_tanh"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "exp")    => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_exp"),    &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "expm1")  => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_expm1"),  &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "log1p")  => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_log1p"),  &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "cbrt")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_cbrt"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "clz32")  => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_clz32"),  &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "fround") => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_fround"), &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "random") => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_random"), &[], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "imul")   => { let a = *arg_vals.first().unwrap_or(&undef_i64); let b = *arg_vals.get(1).unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_imul"), &[a, b], &[i64t], self.loc)).result(0)?.into()) }
                        ("Math", "min")    => { let a = *arg_vals.first().unwrap_or(&undef_i64); let b = *arg_vals.get(1).unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_min"), &[a, b], &[i64t], self.loc)).result(0)?.into()) }
                        ("Math", "max")    => { let a = *arg_vals.first().unwrap_or(&undef_i64); let b = *arg_vals.get(1).unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_max"), &[a, b], &[i64t], self.loc)).result(0)?.into()) }
                        ("Math", "pow")    => { let a = *arg_vals.first().unwrap_or(&undef_i64); let b = *arg_vals.get(1).unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_pow"), &[a, b], &[i64t], self.loc)).result(0)?.into()) }
                        ("Math", "atan2")  => { let a = *arg_vals.first().unwrap_or(&undef_i64); let b = *arg_vals.get(1).unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_atan2"), &[a, b], &[i64t], self.loc)).result(0)?.into()) }
                        ("Math", "hypot")  => { let a = *arg_vals.first().unwrap_or(&undef_i64); let b = *arg_vals.get(1).unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_hypot"), &[a, b], &[i64t], self.loc)).result(0)?.into()) }
                        // ── Object ────────────────────────────────────────────
                        ("Object", "keys")                 => { let o = *arg_vals.first().unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_keys"),         &[o], &[i64t], self.loc)).result(0)?.into()) }
                        ("Object", "values")               => { let o = *arg_vals.first().unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_values"),       &[o], &[i64t], self.loc)).result(0)?.into()) }
                        ("Object", "entries")              => { let o = *arg_vals.first().unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_entries"),      &[o], &[i64t], self.loc)).result(0)?.into()) }
                        ("Object", "assign")               => { let t = *arg_vals.first().unwrap_or(&undef_i64); let s = *arg_vals.get(1).unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_assign"),       &[t, s], &[i64t], self.loc)).result(0)?.into()) }
                        ("Object", "create")               => { let p = *arg_vals.first().unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_create"),       &[p], &[i64t], self.loc)).result(0)?.into()) }
                        ("Object", "fromEntries")          => { let a = *arg_vals.first().unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_from_entries"), &[a], &[i64t], self.loc)).result(0)?.into()) }
                        ("Object", "getOwnPropertyNames")  => { let o = *arg_vals.first().unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get_own_property_names"), &[o], &[i64t], self.loc)).result(0)?.into()) }
                        ("Object", "getPrototypeOf")       => { let o = *arg_vals.first().unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get_prototype_of"), &[o], &[i64t], self.loc)).result(0)?.into()) }
                        // Object.freeze/seal/defineProperty — return the object as-is (no immutability enforcement)
                        ("Object", "freeze") | ("Object", "seal") => {
                            let o = *arg_vals.first().unwrap_or(&undef_i64);
                            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"), &[o], &[], self.loc));
                            Some(o)
                        }
                        ("Object", "isFrozen") | ("Object", "isSealed") => {
                            Some(block.append_operation(arith::constant(self.ctx,
                                IntegerAttribute::new(i64t, (0x7FFA_0000_0000_0001u64) as i64).into(), self.loc,
                            )).result(0)?.into())
                        }
                        ("Object", "is") => {
                            let a = *arg_vals.first().unwrap_or(&undef_i64);
                            let b = *arg_vals.get(1).unwrap_or(&undef_i64);
                            let cmp: Value<'c, 'b> = block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_strict_eq"), &[a, b], &[self.i32_type()], self.loc)).result(0)?.into();
                            Some(self.ensure_i64(cmp, block)?)
                        }
                        ("Object", "hasOwn") => {
                            let o = *arg_vals.first().unwrap_or(&undef_i64);
                            let k = *arg_vals.get(1).unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_has_key"), &[o, k], &[i64t], self.loc)).result(0)?.into())
                        }
                        ("Object", "defineProperty") => {
                            // Object.defineProperty(obj, key, descriptor) — extract .value and set it
                            let o = *arg_vals.first().unwrap_or(&undef_i64);
                            let k = *arg_vals.get(1).unwrap_or(&undef_i64);
                            let desc = *arg_vals.get(2).unwrap_or(&undef_i64);
                            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_define_property"), &[o, k, desc], &[], self.loc));
                            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"), &[o], &[], self.loc));
                            Some(o)
                        }
                        // ── Reflect ───────────────────────────────────────────
                        ("Reflect", "metadata") => {
                            // Reflect.metadata(key, value) → decorator factory
                            // Returns a TsFunction that, when called as decorator(target, propKey?),
                            // calls Reflect.defineMetadata(key, value, target, propKey).
                            let k = *arg_vals.first().unwrap_or(&undef_i64);
                            let v = *arg_vals.get(1).unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_reflect_metadata_decorator"), &[k, v], &[i64t], self.loc)).result(0)?.into())
                        }
                        ("Reflect", "defineMetadata") => {
                            let k  = *arg_vals.first().unwrap_or(&undef_i64);
                            let v  = *arg_vals.get(1).unwrap_or(&undef_i64);
                            let t  = *arg_vals.get(2).unwrap_or(&undef_i64);
                            let pk = *arg_vals.get(3).unwrap_or(&undef_i64);
                            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_reflect_define_metadata"), &[k, v, t, pk], &[], self.loc));
                            Some(undef_i64)
                        }
                        ("Reflect", "getMetadata") => {
                            let k  = *arg_vals.first().unwrap_or(&undef_i64);
                            let t  = *arg_vals.get(1).unwrap_or(&undef_i64);
                            let pk = *arg_vals.get(2).unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_reflect_get_metadata"), &[k, t, pk], &[i64t], self.loc)).result(0)?.into())
                        }
                        ("Reflect", "getOwnMetadata") => {
                            let k  = *arg_vals.first().unwrap_or(&undef_i64);
                            let t  = *arg_vals.get(1).unwrap_or(&undef_i64);
                            let pk = *arg_vals.get(2).unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_reflect_get_own_metadata"), &[k, t, pk], &[i64t], self.loc)).result(0)?.into())
                        }
                        ("Reflect", "hasMetadata") => {
                            let k  = *arg_vals.first().unwrap_or(&undef_i64);
                            let t  = *arg_vals.get(1).unwrap_or(&undef_i64);
                            let pk = *arg_vals.get(2).unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_reflect_has_metadata"), &[k, t, pk], &[i64t], self.loc)).result(0)?.into())
                        }
                        ("Reflect", "hasOwnMetadata") => {
                            let k  = *arg_vals.first().unwrap_or(&undef_i64);
                            let t  = *arg_vals.get(1).unwrap_or(&undef_i64);
                            let pk = *arg_vals.get(2).unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_reflect_has_own_metadata"), &[k, t, pk], &[i64t], self.loc)).result(0)?.into())
                        }
                        ("Reflect", "getMetadataKeys") => {
                            let t  = *arg_vals.first().unwrap_or(&undef_i64);
                            let pk = *arg_vals.get(1).unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_reflect_get_metadata_keys"), &[t, pk], &[i64t], self.loc)).result(0)?.into())
                        }
                        ("Reflect", "getOwnMetadataKeys") => {
                            let t  = *arg_vals.first().unwrap_or(&undef_i64);
                            let pk = *arg_vals.get(1).unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_reflect_get_own_metadata_keys"), &[t, pk], &[i64t], self.loc)).result(0)?.into())
                        }
                        ("Reflect", "deleteMetadata") => {
                            let k  = *arg_vals.first().unwrap_or(&undef_i64);
                            let t  = *arg_vals.get(1).unwrap_or(&undef_i64);
                            let pk = *arg_vals.get(2).unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_reflect_delete_metadata"), &[k, t, pk], &[i64t], self.loc)).result(0)?.into())
                        }
                        ("Reflect", "getPrototypeOf") => {
                            let o = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_reflect_get_prototype_of"), &[o], &[i64t], self.loc)).result(0)?.into())
                        }
                        ("Reflect", "getOwnPropertyDescriptor") => {
                            let o = *arg_vals.first().unwrap_or(&undef_i64);
                            let k = *arg_vals.get(1).unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_reflect_get_own_property_descriptor"), &[o, k], &[i64t], self.loc)).result(0)?.into())
                        }
                        ("Reflect", "ownKeys") => {
                            // Same as Object.getOwnPropertyNames: returns all own keys as array.
                            let o = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get_own_property_names"), &[o], &[i64t], self.loc)).result(0)?.into())
                        }
                        // ── Array ─────────────────────────────────────────────
                        // ── String ────────────────────────────────────────────
                        ("String", "fromCharCode") => {
                            let code = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_from_char_code"),
                                &[code], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        // ── Array ─────────────────────────────────────────────
                        ("Array", "from") => {
                            let iterable = *arg_vals.first().unwrap_or(&undef_i64);
                            // Second arg is optional mapFn; pass UNDEFINED when absent.
                            let map_fn = *arg_vals.get(1).unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_from"),
                                &[iterable, map_fn], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        ("Array", "isArray") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            let flag: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_array"),
                                &[v], &[self.i32_type()], self.loc,
                            )).result(0)?.into();
                            // Wrap i32 result as i1 then as NaN-boxed boolean
                            let i1_val = self.ensure_i1(flag, block)?;
                            Some(self.ensure_i64(i1_val, block)?)
                        }
                        ("Array", "of") => {
                            // Array.of(a, b, c) → create a TsArray with exactly those elements
                            let n = arg_vals.len() as i32;
                            let n_val: Value<'c, 'b> = block.append_operation(arith::constant(
                                self.ctx, IntegerAttribute::new(self.i32_type(), n as i64).into(), self.loc,
                            )).result(0)?.into();
                            let arr: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
                                &[n_val], &[i64t], self.loc,
                            )).result(0)?.into();
                            for (idx, &av) in arg_vals.iter().enumerate() {
                                let idx_val: Value<'c, 'b> = block.append_operation(arith::constant(
                                    self.ctx, IntegerAttribute::new(self.i32_type(), idx as i64).into(), self.loc,
                                )).result(0)?.into();
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_set"),
                                    &[arr, idx_val, av], &[], self.loc,
                                ));
                            }
                            Some(arr)
                        }
                        // ── JSON ──────────────────────────────────────────────
                        ("JSON", "stringify") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_json_stringify"),
                                &[v], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        ("JSON", "parse") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_json_parse"),
                                &[v], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        // ── Promise ───────────────────────────────────────────
                        ("Promise", "resolve") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_promise_resolve"),
                                &[v], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        ("Promise", "reject") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_promise_reject"),
                                &[v], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        ("Promise", "all") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_promise_all"),
                                &[v], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        ("Promise", "allSettled") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_promise_all_settled"),
                                &[v], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        ("Promise", "any") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_promise_any"),
                                &[v], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        ("Promise", "race") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_promise_race_all"),
                                &[v], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        // ── Date static ───────────────────────────────────────
                        ("Date", "now") => {
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_now"),
                                &[], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        // ── Number static ─────────────────────────────────────
                        ("Number", "isInteger") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_number_is_integer"),
                                &[v], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        ("Number", "isFinite") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_number_is_finite"),
                                &[v], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        ("Number", "isNaN") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_number_is_nan"),
                                &[v], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        ("Number", "isSafeInteger") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_number_is_safe_integer"),
                                &[v], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        ("Number", "parseInt") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            let radix = *arg_vals.get(1).unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_parse_int"),
                                &[v, radix], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        ("Number", "parseFloat") => {
                            let v = *arg_vals.first().unwrap_or(&undef_i64);
                            Some(block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_parse_float"),
                                &[v], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                        _ => None,
                    };

                    for av in &arg_vals {
                        block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                            &[*av], &[], self.loc,
                        ));
                    }

                    if result_opt.is_some() || matches!((ns, method.as_str()),
                        ("Math", "abs"|"floor"|"ceil"|"round"|"sqrt"|"trunc"|"log"|"log2"|"log10"|
                                 "sin"|"cos"|"tan"|"sign"|"asin"|"acos"|"atan"|"sinh"|"cosh"|"tanh"|
                                 "exp"|"expm1"|"log1p"|"cbrt"|"clz32"|"fround"|"random"|"imul"|
                                 "min"|"max"|"pow"|"atan2"|"hypot") |
                        ("Object", "keys"|"values"|"entries"|"assign"|"create"|"fromEntries") |
                        ("Array", "isArray"|"of"|"from") |
                        ("String", "fromCharCode") |
                        ("JSON", "stringify"|"parse") |
                        ("Promise", "resolve"|"reject"|"all"|"allSettled"|"any"|"race") |
                        ("Date", "now") |
                        ("Number", "isInteger"|"isFinite"|"isNaN"|"isSafeInteger"|"parseInt"|"parseFloat")
                    ) {
                        return Ok((result_opt, block));
                    }
                }
            }
        }

        // Method call: obj.method(args)  → __class_Foo_method(obj, args)
        if let Expression::StaticMemberExpression(member) = &call.callee {
            let method_name = member.property.name.as_str().to_string();

            // ── Static method call: ClassName.staticMethod(args) ────────────────
            if let Expression::Identifier(id) = &member.object {
                if let Some(class_sig) = self.classes.get(id.name.as_str()).cloned() {
                    if let Some(mangled) = class_sig.statics.get(&method_name).cloned() {
                        let mut args: Vec<Value<'c, 'b>> = Vec::new();
                        for arg in &call.arguments {
                            let expr = arg.as_expression()
                                .ok_or_else(|| anyhow::anyhow!("spread in static method call"))?;
                            let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            let v = v_opt.ok_or_else(|| anyhow::anyhow!("static method arg produced no value"))?;
                            args.push(self.ensure_i64(v, block)?);
                        }
                        // Pad with UNDEFINED / truncate to match the static method's MLIR signature.
                        if let Some(fn_sig) = self.funcs.get(&mangled).cloned() {
                            let expected = fn_sig.param_types.len();
                            let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                                self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                            )).result(0)?.into();
                            while args.len() < expected { args.push(undef_i64); }
                            args.truncate(expected);
                        }
                        let op = block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, &mangled),
                            &args,
                            &[self.i64_type()],
                            self.loc,
                        ));
                        return Ok((Some(op.result(0)?.into()), block));
                    }
                }
            }

            // ── Instance method dispatch ─────────────────────────────────────────
            // Determine the object's class from var_class_types (best-effort inference).
            let class_name_opt = if let Expression::Identifier(id) = &member.object {
                self.var_class_types.get(id.name.as_str()).cloned()
            } else {
                None
            };

            if let Some(class_name) = class_name_opt {
                let method_name = member.property.name.as_str().to_string();
                if let Some(sig) = self.classes.get(&class_name).cloned() {
                    if let Some(mangled) = sig.methods.get(&method_name).cloned() {
                        // Lower `this` object.
                        let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
                        block = nb;
                        let obj = match obj_opt {
                            Some(v) => v,
                            None => { let u: Value<'c,'b> = block.append_operation(arith::constant(self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc)).result(0)?.into(); u }
                        };
                        let obj_i64 = self.ensure_i64(obj, block)?;

                        // Lower arguments. If any spread is present, fall through to dynamic dispatch.
                        let has_spread = call.arguments.iter().any(|a| matches!(a, oxc_ast::ast::Argument::SpreadElement(_)));
                        if has_spread {
                            // Fall through to dynamic dispatch below.
                        } else {
                        let method_rest = sig.method_has_rest.contains(&method_name);
                        let expected_arity = sig.method_arity.get(&method_name).copied().unwrap_or(0);
                        // n_regular = total arity minus self minus rest slot (if any)
                        let n_regular_params = if method_rest {
                            expected_arity.saturating_sub(2) // subtract self + rest slot
                        } else {
                            expected_arity.saturating_sub(1) // subtract self
                        };

                        let mut args = vec![obj_i64];
                        for arg in &call.arguments {
                            let expr = arg.as_expression()
                                .ok_or_else(|| anyhow::anyhow!("spread in method call"))?;
                            let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            let v = v_opt
                                .ok_or_else(|| anyhow::anyhow!("method arg produced no value"))?;
                            args.push(self.ensure_i64(v, block)?);
                        }

                        if method_rest {
                            // Bundle excess args (past n_regular_params) into a TsArray rest param.
                            let n_provided = args.len() - 1; // exclude self
                            let i32_type = self.i32_type();
                            // Pad regular params with UNDEFINED if underprovided.
                            while args.len() < 1 + n_regular_params {
                                let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                                    self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                                )).result(0)?.into();
                                args.push(undef);
                            }
                            // Build rest array from excess args.
                            let rest_slice: Vec<Value<'c, 'b>> = if n_provided > n_regular_params {
                                args.drain(1 + n_regular_params..).collect()
                            } else {
                                vec![]
                            };
                            let n_rest = rest_slice.len() as i32;
                            let n_val: Value<'c, 'b> = block.append_operation(arith::constant(
                                self.ctx, IntegerAttribute::new(i32_type, n_rest as i64).into(), self.loc,
                            )).result(0)?.into();
                            let rest_arr: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
                                &[n_val], &[self.i64_type()], self.loc,
                            )).result(0)?.into();
                            for (idx, rv) in rest_slice.into_iter().enumerate() {
                                let idx_val: Value<'c, 'b> = block.append_operation(arith::constant(
                                    self.ctx, IntegerAttribute::new(i32_type, idx as i64).into(), self.loc,
                                )).result(0)?.into();
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_set"),
                                    &[rest_arr, idx_val, rv], &[], self.loc,
                                ));
                            }
                            args.push(rest_arr);
                        } else {
                            // Pad with UNDEFINED if fewer args than expected.
                            while args.len() < expected_arity {
                                let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                                    self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                                )).result(0)?.into();
                                args.push(undef);
                            }
                        }

                        let result_types = &[self.i64_type()];
                        let op = block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, &mangled),
                            &args,
                            result_types,
                            self.loc,
                        ));
                        return Ok((Some(op.result(0)?.into()), block));
                        } // end if !has_spread
                    }
                }
            }
        }

        // ── Built-in array / string method dispatch ──────────────────────────
        if let Expression::StaticMemberExpression(member) = &call.callee {
            let method_name = member.property.name.as_str().to_string();
            // If the receiver is a known user-defined class instance, skip builtin dispatch
            // and fall through to dynamic method dispatch below.  This prevents e.g.
            // `app.get(path, handler)` from being mistakenly compiled as ts_map_get.
            let receiver_is_user_class_identifier = if let Expression::Identifier(id) = &member.object {
                self.var_class_types
                    .get(id.name.as_str())
                    .map(|cn| self.classes.contains_key(cn.as_str()))
                    .unwrap_or(false)
            } else {
                false
            };
            // For chained member access rooted at `this` (e.g. `this.repo.find()`), the
            // intermediate field could be a user-class instance. Skip builtin dispatch for
            // ambiguous method names that are commonly user-defined AND the chain root is `this`.
            // This preserves `url.searchParams.get()` (root = url, not this) as builtin.
            // When the receiver is `this.FIELD`, check if FIELD is a known user-class instance
            // (from constructor parameter property type annotations). If so, skip builtin dispatch
            // so user-defined methods on the field are called dynamically.
            // This correctly handles `this.repo.find()` (repo: Repo) without breaking
            // `this.routes.forEach(cb)` (routes: array, no type annotation → uses builtin).
            let receiver_is_this_field_user_class = if let Expression::StaticMemberExpression(inner) = &member.object {
                if matches!(&inner.object, Expression::ThisExpression(_)) {
                    let field_name = inner.property.name.as_str();
                    self.current_class.as_ref().and_then(|cn| self.classes.get(cn.as_str())).map(|sig| {
                        sig.field_class_types.get(field_name)
                            .map(|class_type| self.classes.contains_key(class_type.as_str()))
                            .unwrap_or(false)
                    }).unwrap_or(false)
                } else { false }
            } else { false };
            let receiver_is_user_class = receiver_is_user_class_identifier || receiver_is_this_field_user_class;
            let n_call_args = call.arguments.len();
            let is_builtin = !receiver_is_user_class && (
                // "add" is a container op only with 1 arg (Set.add/WeakSet.add);
                // with more args it's a user-defined method (e.g. router.add(method, path, arr)).
                (method_name == "add" && n_call_args == 1) ||
                // "match" is a string builtin only with 1 arg (String.prototype.match(re));
                // with 2+ args it's a user method (e.g. router.match(method, path)).
                (method_name == "match" && n_call_args == 1) ||
                (method_name == "matchAll" && n_call_args == 1) ||
                // "toString" with 0 args → generic ts_val_to_string (handles numbers/booleans/strings).
                // With 1+ args (e.g. Buffer.toString('utf8', start, end)) → dynamic dispatch to class method.
                (method_name == "toString" && n_call_args == 0) ||
                matches!(method_name.as_str(),
                "push" | "pop" | "unshift" | "shift" | "indexOf" | "lastIndexOf" | "includes" | "join" |
                "slice" | "substring" | "concat" | "toUpperCase" | "toLowerCase" | "trim" | "split" |
                // Array HOFs
                "map" | "filter" | "forEach" | "reduce" | "reduceRight" | "find" |
                "findIndex" | "findLast" | "findLastIndex" | "some" | "every" | "sort" | "flatMap" | "flat" |
                "toSorted" | "toReversed" | "with" |
                "search" |
                // Array mutating methods
                "reverse" | "fill" | "splice" | "copyWithin" | "lastIndexOf" |
                // String methods
                "replace" | "replaceAll" | "startsWith" | "endsWith" |
                "padStart" | "padEnd" | "charAt" | "charCodeAt" | "repeat" | "at" | "localeCompare" |
                "toFixed" | "toPrecision" | "toExponential" |
                "hasOwnProperty" |
                // Container methods (Map, Set, WeakMap, WeakSet) — "add" handled above
                "set" | "get" | "has" | "delete" | "clear" | "keys" | "values" | "entries" |
                // RegExp methods
                "test" | "exec" |
                // Request/Response body
                "text" | "json" |
                // String trim variants
                "trimStart" | "trimEnd" | "trimLeft" | "trimRight" |
                // URLSearchParams / generic no-arg toString (with args → dynamic dispatch for Buffer.toString(enc,start,end))
                "getAll" |
                // Date instance methods
                "getTime" | "getFullYear" | "getMonth" | "getDate" | "getDay" |
                "getHours" | "getMinutes" | "getSeconds" | "getMilliseconds" |
                "toISOString" | "toLocaleDateString" | "toLocaleTimeString" | "toLocaleString" |
                // WeakRef
                "deref" |
                // Promise chaining
                "then" | "catch" | "finally"
            ));
            if is_builtin {
                // If any argument is a spread, handle specially or fall through to dynamic.
                let has_spread = call.arguments.iter().any(|a| matches!(a, oxc_ast::ast::Argument::SpreadElement(_)));
                if has_spread && method_name != "push" {
                    // Fall through to dynamic dispatch for spread args on non-push builtins.
                } else {
                // Evaluate receiver
                let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
                block = nb;
                let obj = match obj_opt {
                    Some(v) => v,
                    None => { let u: Value<'c,'b> = block.append_operation(arith::constant(self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc)).result(0)?.into(); u }
                };
                let obj_i64 = self.ensure_i64(obj, block)?;

                if has_spread {
                    // push(...arr) → push_all for each spread arg
                    let i64t = self.i64_type();
                    let mut result_val: Option<Value<'c, 'b>> = None;
                    for arg in &call.arguments {
                        match arg {
                            oxc_ast::ast::Argument::SpreadElement(spread) => {
                                let (v_opt, nb) = self.lower_expression(&spread.argument, block, region, scope)?;
                                block = nb;
                                let arr = v_opt.ok_or_else(|| anyhow::anyhow!("push spread: no value"))?;
                                let arr_i64 = self.ensure_i64(arr, block)?;
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push_all"),
                                    &[obj_i64, arr_i64], &[], self.loc,
                                ));
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                    &[arr_i64], &[], self.loc,
                                ));
                            }
                            _ => {
                                let expr = arg.as_expression().ok_or_else(|| anyhow::anyhow!("push arg error"))?;
                                let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                                block = nb;
                                let v = v_opt.ok_or_else(|| anyhow::anyhow!("push arg no value"))?;
                                let v_i64 = self.ensure_i64(v, block)?;
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push"),
                                    &[obj_i64, v_i64], &[], self.loc,
                                ));
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                    &[v_i64], &[], self.loc,
                                ));
                                result_val = Some(block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_len"),
                                    &[obj_i64], &[i64t], self.loc,
                                )).result(0)?.into());
                            }
                        }
                    }
                    // push with no regular args returns length; use arr_len if no result set
                    if result_val.is_none() {
                        result_val = Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_len"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into());
                    }
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[obj_i64], &[], self.loc,
                    ));
                    return Ok((result_val, block));
                }

                // Evaluate all arguments (no spread here)
                let mut arg_vals: Vec<Value<'c, 'b>> = Vec::new();
                for arg in &call.arguments {
                    if let Some(expr) = arg.as_expression() {
                        let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                        block = nb;
                        if let Some(v) = v_opt {
                            arg_vals.push(self.ensure_i64(v, block)?);
                        }
                    }
                }

                // Dispatch to the appropriate runtime function
                let i64t = self.i64_type();
                let undefined_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx,
                    IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(),
                    self.loc,
                )).result(0)?.into();

                let result: Option<Value<'c, 'b>> = match method_name.as_str() {
                    "push" => {
                        let val = arg_vals.first().copied().unwrap_or(undefined_i64);
                        block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push"),
                            &[obj_i64, val], &[], self.loc,
                        ));
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_len"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "pop" => {
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_pop"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "unshift" => {
                        let val = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_unshift"),
                            &[obj_i64, val], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "shift" => {
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_shift"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "indexOf" => {
                        let search = arg_vals.first().copied().unwrap_or(undefined_i64);
                        if let Some(&from) = arg_vals.get(1) {
                            Some(block.append_operation(func::call(
                                self.ctx,
                                FlatSymbolRefAttribute::new(self.ctx, "ts_str_index_of_from"),
                                &[obj_i64, search, from], &[i64t], self.loc,
                            )).result(0)?.into())
                        } else {
                            Some(block.append_operation(func::call(
                                self.ctx,
                                FlatSymbolRefAttribute::new(self.ctx, "ts_val_index_of"),
                                &[obj_i64, search], &[i64t], self.loc,
                            )).result(0)?.into())
                        }
                    }
                    "lastIndexOf" => {
                        let search = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_str_last_index_of"),
                            &[obj_i64, search], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "includes" => {
                        let search = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_val_includes"),
                            &[obj_i64, search], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "join" => {
                        let sep = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_join"),
                            &[obj_i64, sep], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "slice" => {
                        let start = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let end   = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_str_slice"),
                            &[obj_i64, start, end], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "substring" => {
                        let start = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let end   = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_str_substring"),
                            &[obj_i64, start, end], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "toUpperCase" => {
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_str_to_upper"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "toLowerCase" => {
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_str_to_lower"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "trim" => {
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_str_trim"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "split" => {
                        let sep = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_str_split"),
                            &[obj_i64, sep], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    // Array HOFs
                    "map" => {
                        let cb = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_map"),
                            &[obj_i64, cb], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "filter" => {
                        let cb = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_filter"),
                            &[obj_i64, cb], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "forEach" => {
                        let cb = arg_vals.first().copied().unwrap_or(undefined_i64);
                        block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_container_for_each"),
                            &[obj_i64, cb], &[i64t], self.loc,
                        ));
                        None
                    }
                    "reduce" => {
                        let cb   = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let init = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_reduce"),
                            &[obj_i64, cb, init], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "reduceRight" => {
                        let cb   = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let init = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_reduce_right"),
                            &[obj_i64, cb, init], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "find" => {
                        let cb = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_find"),
                            &[obj_i64, cb], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "findIndex" => {
                        let cb = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_find_index"),
                            &[obj_i64, cb], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "findLast" => {
                        let cb = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_find_last"),
                            &[obj_i64, cb], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "findLastIndex" => {
                        let cb = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_find_last_index"),
                            &[obj_i64, cb], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "some" => {
                        let cb = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_some"),
                            &[obj_i64, cb], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "every" => {
                        let cb = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_every"),
                            &[obj_i64, cb], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "sort" => {
                        let cb = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_sort"),
                            &[obj_i64, cb], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "toSorted" => {
                        let cb = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_to_sorted"),
                            &[obj_i64, cb], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "toReversed" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_to_reversed"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "with" => {
                        let idx = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let val = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_with"),
                            &[obj_i64, idx, val], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "search" => {
                        let re = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_search"),
                            &[obj_i64, re], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "hasOwnProperty" => {
                        let key = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_has_key"),
                            &[obj_i64, key], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "flatMap" => {
                        let cb = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_flat_map"),
                            &[obj_i64, cb], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "concat" => {
                        let other = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_concat"),
                            &[obj_i64, other], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "reverse" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_reverse"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "fill" => {
                        let value      = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let start      = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        let end        = arg_vals.get(2).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_fill"),
                            &[obj_i64, value, start, end], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "splice" => {
                        let start        = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let delete_count = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_splice"),
                            &[obj_i64, start, delete_count], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "copyWithin" => {
                        let target = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let start  = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        let end    = arg_vals.get(2).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_copy_within"),
                            &[obj_i64, target, start, end], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "lastIndexOf" => {
                        let val = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_last_index_of"),
                            &[obj_i64, val], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "flat" => {
                        // depth arg: default 1; extract i32 from NaN-boxed i64
                        let depth_i64 = arg_vals.first().copied().unwrap_or_else(|| {
                            // NaN-boxed integer 1: TAG_INT | 1
                            block.append_operation(arith::constant(
                                self.ctx,
                                IntegerAttribute::new(i64t, (0x7FFE_0000_0000_0000u64 | 1) as i64).into(),
                                self.loc,
                            )).result(0).unwrap().into()
                        });
                        let depth_i32 = self.ensure_i32(depth_i64, block)?;
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_flat"),
                            &[obj_i64, depth_i32], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "replace" => {
                        let search = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let repl   = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        // Use regex variant if first arg might be RegExp (ts_str_replace_regex handles both)
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_replace_regex"),
                            &[obj_i64, search, repl], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "replaceAll" => {
                        let search = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let repl   = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_replace_all"),
                            &[obj_i64, search, repl], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "startsWith" => {
                        let prefix = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_starts_with"),
                            &[obj_i64, prefix], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "endsWith" => {
                        let suffix = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_ends_with"),
                            &[obj_i64, suffix], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "padStart" => {
                        let len  = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let fill = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_pad_start"),
                            &[obj_i64, len, fill], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "padEnd" => {
                        let len  = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let fill = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_pad_end"),
                            &[obj_i64, len, fill], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "charAt" => {
                        let idx = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_char_at"),
                            &[obj_i64, idx], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "charCodeAt" => {
                        let idx = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_char_code_at"),
                            &[obj_i64, idx], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "repeat" => {
                        let count = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_repeat"),
                            &[obj_i64, count], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "at" => {
                        let idx = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_at"),
                            &[obj_i64, idx], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "localeCompare" => {
                        let other = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_locale_compare"),
                            &[obj_i64, other], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "toFixed" => {
                        let digits = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_num_to_fixed"),
                            &[obj_i64, digits], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "toPrecision" => {
                        let prec = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_num_to_precision"),
                            &[obj_i64, prec], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "toExponential" => {
                        let digits = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_num_to_exponential"),
                            &[obj_i64, digits], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    // ── Container methods (Map / Set / WeakMap / WeakSet) ─────
                    "add" => {
                        let val = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_container_add"),
                            &[obj_i64, val], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "set" => {
                        let key = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let val = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_container_set"),
                            &[obj_i64, key, val], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "get" => {
                        let key = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_container_get"),
                            &[obj_i64, key], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "has" => {
                        let key = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_container_has"),
                            &[obj_i64, key], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "delete" => {
                        let key = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_container_delete"),
                            &[obj_i64, key], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "clear" => {
                        block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_container_clear"),
                            &[obj_i64], &[], self.loc,
                        ));
                        None
                    }
                    "keys" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_container_keys"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "values" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_container_values"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "entries" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_container_entries"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    // ── RegExp methods ──────────────────────────────────────
                    "test" => {
                        let s = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_regexp_test"),
                            &[obj_i64, s], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "exec" => {
                        let s = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_regexp_exec"),
                            &[obj_i64, s], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "match" => {
                        // str.match(re): obj=string, arg0=regexp
                        let re = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_match"),
                            &[obj_i64, re], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "matchAll" => {
                        let re = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_match_all"),
                            &[obj_i64, re], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    // ── Headers methods ─────────────────────────────────────
                    "append" => {
                        let name  = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let value = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_headers_append"),
                            &[obj_i64, name, value], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "getSetCookie" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_headers_get_set_cookie"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "get" => {
                        let name = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_headers_get"),
                            &[obj_i64, name], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "has" => {
                        let name = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_headers_has"),
                            &[obj_i64, name], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "set" => {
                        let name  = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let value = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_headers_set"),
                            &[obj_i64, name, value], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "delete" => {
                        let name = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_headers_delete"),
                            &[obj_i64, name], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    // ── Response methods ─────────────────────────────────────
                    "clone" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_response_clone"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    // ── Request/Response body methods ─────────────────────────
                    "text" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_text"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "json" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_json"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    // ── String trim variants ──────────────────────────────────
                    "trimStart" | "trimLeft" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_trim_start"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "trimEnd" | "trimRight" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_str_trim_end"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    // ── URLSearchParams / generic toString ───────────────────
                    "toString" => {
                        // URLSearchParams.toString() → ts_urlsearchparams_to_string;
                        // generic val.toString() → ts_val_to_string (handles number, bool, etc.)
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_to_string"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "getAll" => {
                        let name = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_urlsearchparams_get_all"),
                            &[obj_i64, name], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    // ── Date instance methods ──────────────────────────────────
                    "getTime"              => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_get_time"),              &[obj_i64], &[i64t], self.loc)).result(0)?.into()),
                    "getFullYear"          => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_get_full_year"),         &[obj_i64], &[i64t], self.loc)).result(0)?.into()),
                    "getMonth"             => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_get_month"),             &[obj_i64], &[i64t], self.loc)).result(0)?.into()),
                    "getDate"              => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_get_date"),              &[obj_i64], &[i64t], self.loc)).result(0)?.into()),
                    "getDay"               => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_get_day"),               &[obj_i64], &[i64t], self.loc)).result(0)?.into()),
                    "getHours"             => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_get_hours"),             &[obj_i64], &[i64t], self.loc)).result(0)?.into()),
                    "getMinutes"           => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_get_minutes"),           &[obj_i64], &[i64t], self.loc)).result(0)?.into()),
                    "getSeconds"           => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_get_seconds"),           &[obj_i64], &[i64t], self.loc)).result(0)?.into()),
                    "getMilliseconds"      => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_get_milliseconds"),      &[obj_i64], &[i64t], self.loc)).result(0)?.into()),
                    "toISOString"          => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_to_iso_string"),         &[obj_i64], &[i64t], self.loc)).result(0)?.into()),
                    "toLocaleDateString"   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_to_locale_date_string"), &[obj_i64], &[i64t], self.loc)).result(0)?.into()),
                    "toLocaleTimeString"   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_to_locale_time_string"), &[obj_i64], &[i64t], self.loc)).result(0)?.into()),
                    "toLocaleString"       => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_date_to_string"),             &[obj_i64], &[i64t], self.loc)).result(0)?.into()),
                    // ── WeakRef methods ────────────────────────────────────────
                    "deref" => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_weakref_deref"), &[obj_i64], &[i64t], self.loc)).result(0)?.into()),
                    // ── Promise chaining ───────────────────────────────────────
                    "then" | "catch" | "finally" => {
                        let undef_const: Value<'c, 'b> = block.append_operation(arith::constant(self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc)).result(0)?.into();
                        let cb = *arg_vals.first().unwrap_or(&undef_const);
                        let fn_name = match method_name.as_str() {
                            "then"    => "ts_promise_then",
                            "catch"   => "ts_promise_catch",
                            _         => "ts_promise_finally",
                        };
                        Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, fn_name), &[obj_i64, cb], &[i64t], self.loc)).result(0)?.into())
                    }
                    _ => None,
                };

                // Release receiver and all argument temporaries
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[obj_i64], &[], self.loc,
                ));
                for av in &arg_vals {
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[*av], &[], self.loc,
                    ));
                }

                return Ok((result, block));
                } // end else { // no spread or push-with-spread handled above
            }
        }

        // Special case: fn.bind(thisArg) → ts_func_bind(fn, thisArg)
        if let Expression::StaticMemberExpression(member) = &call.callee {
            if member.property.name.as_str() == "bind" {
                let (fn_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
                block = nb;
                if let Some(fn_val) = fn_opt {
                    let i64t = self.i64_type();
                    let fn_i64 = self.ensure_i64(fn_val, block)?;
                    let undef_i64: Value<'c, 'b> = block.append_operation(arith::constant(
                        self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                    )).result(0)?.into();
                    let this_arg = if let Some(arg) = call.arguments.first() {
                        if let Some(expr) = arg.as_expression() {
                            let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            v_opt.map(|v| self.ensure_i64(v, block)).transpose()?.unwrap_or(undef_i64)
                        } else { undef_i64 }
                    } else { undef_i64 };
                    let bound: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_func_bind"),
                        &[fn_i64, this_arg], &[i64t], self.loc,
                    )).result(0)?.into();
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[fn_i64], &[], self.loc,
                    ));
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[this_arg], &[], self.loc,
                    ));
                    return Ok((Some(bound), block));
                }
            }
        }

        // Generic fallback: evaluate callee as a dynamic function value and dispatch.
        // For member expression callees, use ts_method_callN so that functions with
        // an explicit TypeScript `this` parameter receive the receiver as `this`.
        if let Expression::StaticMemberExpression(member) = &call.callee {
            let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
            block = nb;
            if let Some(obj_val) = obj_opt {
                let i64t = self.i64_type();
                let obj_i64 = self.ensure_i64(obj_val, block)?;
                let key_ptr = self.get_string_ptr(member.property.name.as_str(), block)?;
                let fn_val: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                    &[obj_i64, key_ptr], &[i64t], self.loc,
                )).result(0)?.into();
                let result = self.lower_method_call(fn_val, obj_i64, &call.arguments, block, region, scope)?;
                let (result_val, nb) = result;
                block = nb;
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[fn_val], &[], self.loc,
                ));
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[obj_i64], &[], self.loc,
                ));
                return Ok((result_val, block));
            }
        }
        // Computed member callee: obj[key]() — evaluate obj and key, get fn, then method-call.
        if let Expression::ComputedMemberExpression(member) = &call.callee {
            let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
            block = nb;
            if let Some(obj_val) = obj_opt {
                let i64t = self.i64_type();
                let obj_i64 = self.ensure_i64(obj_val, block)?;
                let (key_opt, nb) = self.lower_expression(&member.expression, block, region, scope)?;
                block = nb;
                if let Some(key_val) = key_opt {
                    let key_i64 = self.ensure_i64(key_val, block)?;
                    let fn_val: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_get_key"),
                        &[obj_i64, key_i64], &[i64t], self.loc,
                    )).result(0)?.into();
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[key_i64], &[], self.loc,
                    ));
                    let result = self.lower_method_call(fn_val, obj_i64, &call.arguments, block, region, scope)?;
                    let (result_val, nb) = result;
                    block = nb;
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[fn_val], &[], self.loc,
                    ));
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[obj_i64], &[], self.loc,
                    ));
                    return Ok((result_val, block));
                }
            }
        }
        let (fn_opt, nb) = self.lower_expression(&call.callee, block, region, scope)?;
        block = nb;
        if let Some(fn_val) = fn_opt {
            return self.lower_dynamic_call(fn_val, &call.arguments, block, region, scope);
        }

        tracing::debug!("skipping unimplemented call expression");
        Ok((None, block))
    }

    // ── Method call dispatch: passes receiver as `this` if has_this=1 ────

    pub(super) fn lower_method_call<'b>(
        &mut self,
        fn_val: Value<'c, 'b>,
        obj_val: Value<'c, 'b>,
        arguments: &[oxc_ast::ast::Argument<'_>],
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let i64t = self.i64_type();
        let fn_i64 = self.ensure_i64(fn_val, block)?;

        let has_spread = arguments.iter().any(|a| matches!(a, oxc_ast::ast::Argument::SpreadElement(_)));

        if has_spread {
            // Build an args array, push normal args and spread the spread arrays, then call
            // ts_method_spread_call(fn, obj, args_arr).
            let zero_i32: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(self.i32_type(), 0).into(), self.loc,
            )).result(0)?.into();
            let args_arr: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
                &[zero_i32], &[i64t], self.loc,
            )).result(0)?.into();
            let mut temp_vals: Vec<Value<'c, 'b>> = Vec::new();
            for arg in arguments {
                match arg {
                    oxc_ast::ast::Argument::SpreadElement(spread) => {
                        let (v_opt, nb) = self.lower_expression(&spread.argument, block, region, scope)?;
                        block = nb;
                        if let Some(v) = v_opt {
                            let v_i64 = self.ensure_i64(v, block)?;
                            block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push_all"),
                                &[args_arr, v_i64], &[], self.loc,
                            ));
                            temp_vals.push(v_i64);
                        }
                    }
                    _ => {
                        if let Some(expr) = arg.as_expression() {
                            let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            if let Some(v) = v_opt {
                                let v_i64 = self.ensure_i64(v, block)?;
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push"),
                                    &[args_arr, v_i64], &[], self.loc,
                                ));
                                temp_vals.push(v_i64);
                            }
                        }
                    }
                }
            }
            let result: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_method_spread_call"),
                &[fn_i64, obj_val, args_arr], &[i64t], self.loc,
            )).result(0)?.into();
            for tv in &temp_vals {
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[*tv], &[], self.loc,
                ));
            }
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[args_arr], &[], self.loc,
            ));
            return Ok((Some(result), block));
        }

        // No spread: evaluate args then call ts_method_callN.
        let mut arg_vals: Vec<Value<'c, 'b>> = Vec::new();
        for arg in arguments {
            if let Some(expr) = arg.as_expression() {
                let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                block = nb;
                if let Some(v) = v_opt {
                    arg_vals.push(self.ensure_i64(v, block)?);
                }
            }
        }
        let call_fn = match arg_vals.len() {
            0 => "ts_method_call0",
            1 => "ts_method_call1",
            2 => "ts_method_call2",
            3 => "ts_method_call3",
            4 => "ts_method_call4",
            5 => "ts_method_call5",
            6 => "ts_method_call6",
            7 => "ts_method_call7",
            _ => "ts_method_call8",
        };
        let max_args = arg_vals.len().min(8);
        let mut call_args = vec![fn_i64, obj_val];
        call_args.extend(arg_vals.iter().take(max_args).copied());
        let result: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, call_fn),
            &call_args, &[i64t], self.loc,
        )).result(0)?.into();
        for av in &arg_vals {
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[*av], &[], self.loc,
            ));
        }
        Ok((Some(result), block))
    }

    // ── Dynamic function dispatch (fn_val)(args) ─────────────────────────

    pub(super) fn lower_dynamic_call<'b>(
        &mut self,
        fn_val: Value<'c, 'b>,
        arguments: &[oxc_ast::ast::Argument<'_>],
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let i64t = self.i64_type();
        let fn_i64 = self.ensure_i64(fn_val, block)?;
        block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"),
            &[fn_i64], &[], self.loc,
        ));

        let has_spread = arguments.iter().any(|a| matches!(a, oxc_ast::ast::Argument::SpreadElement(_)));

        if has_spread {
            let zero_i32: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(self.i32_type(), 0).into(), self.loc,
            )).result(0)?.into();
            let args_arr: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
                &[zero_i32], &[i64t], self.loc,
            )).result(0)?.into();
            let mut temp_vals: Vec<Value<'c, 'b>> = Vec::new();
            for arg in arguments {
                match arg {
                    oxc_ast::ast::Argument::SpreadElement(spread) => {
                        let (v_opt, nb) = self.lower_expression(&spread.argument, block, region, scope)?;
                        block = nb;
                        if let Some(v) = v_opt {
                            let v_i64 = self.ensure_i64(v, block)?;
                            block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push_all"),
                                &[args_arr, v_i64], &[], self.loc,
                            ));
                            temp_vals.push(v_i64);
                        }
                    }
                    _ => {
                        if let Some(expr) = arg.as_expression() {
                            let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                            block = nb;
                            if let Some(v) = v_opt {
                                let v_i64 = self.ensure_i64(v, block)?;
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push"),
                                    &[args_arr, v_i64], &[], self.loc,
                                ));
                                temp_vals.push(v_i64);
                            }
                        }
                    }
                }
            }
            let result: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_func_spread_call"),
                &[fn_i64, args_arr], &[i64t], self.loc,
            )).result(0)?.into();
            for tv in &temp_vals {
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[*tv], &[], self.loc,
                ));
            }
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[args_arr], &[], self.loc,
            ));
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[fn_i64], &[], self.loc,
            ));
            return Ok((Some(result), block));
        }

        // No spread: evaluate args then call ts_func_callN
        let mut arg_vals: Vec<Value<'c, 'b>> = Vec::new();
        for arg in arguments {
            if let Some(expr) = arg.as_expression() {
                let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                block = nb;
                if let Some(v) = v_opt {
                    arg_vals.push(self.ensure_i64(v, block)?);
                }
            }
        }
        let call_fn = match arg_vals.len() {
            0 => "ts_func_call0",
            1 => "ts_func_call1",
            2 => "ts_func_call2",
            3 => "ts_func_call3",
            _ => "ts_func_call4",
        };
        let mut call_args = vec![fn_i64];
        call_args.extend(arg_vals.iter().take(4).copied());
        let result: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, call_fn),
            &call_args, &[i64t], self.loc,
        )).result(0)?.into();
        for av in &arg_vals {
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[*av], &[], self.loc,
            ));
        }
        block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[fn_i64], &[], self.loc,
        ));
        Ok((Some(result), block))
    }

}
