use super::*;

impl<'c, 'm> Lowerer<'c, 'm> {

    // ── Expression lowering ───────────────────────────────────────────────

    pub(super) fn lower_expression<'b>(
        &mut self,
        expr: &Expression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        match expr {
            Expression::NumericLiteral(num) => {
                Ok((Some(self.lower_numeric_literal(num.value as i64, block)?), block))
            }
            Expression::BooleanLiteral(b) => {
                Ok((Some(self.lower_boolean_literal(b.value, block)?), block))
            }
            Expression::StringLiteral(s) => {
                Ok((Some(self.lower_string_literal(s.value.as_str(), block)?), block))
            }
            Expression::ArrayExpression(array) => {
                self.lower_array_expression(array, block, region, scope)
            }
            Expression::ObjectExpression(obj) => {
                self.lower_object_expression(obj, block, region, scope)
            }
            Expression::ComputedMemberExpression(member) => {
                self.lower_computed_member_expression(member, block, region, scope)
            }
            Expression::TSAsExpression(ts_as) => {
                self.lower_expression(&ts_as.expression, block, region, scope)
            }
            Expression::TSSatisfiesExpression(ts_sat) => {
                self.lower_expression(&ts_sat.expression, block, region, scope)
            }
            Expression::TSTypeAssertion(ts_assert) => {
                self.lower_expression(&ts_assert.expression, block, region, scope)
            }
            Expression::StaticMemberExpression(member) => {
                // Enum constant access (e.g., Direction.Up).
                if let Expression::Identifier(obj_id) = &member.object {
                    let obj_name = obj_id.name.as_str();
                    let prop_name = member.property.name.as_str();
                    if let Some(enum_members) = self.enums.get(obj_name) {
                        if let Some(&val) = enum_members.get(prop_name) {
                            let lit = self.lower_numeric_literal(val, block)?;
                            return Ok((Some(lit), block));
                        }
                    }
                }

                // Check for getter dispatch before lowering the object.
                let prop_name = member.property.name.to_string();
                let getter_mangled: Option<String> = if let Expression::Identifier(id) = &member.object {
                    let class_name_opt = self.var_class_types.get(id.name.as_str()).cloned();
                    class_name_opt.and_then(|cn| {
                        self.classes.get(&cn).and_then(|sig| {
                            if sig.getters.contains(&prop_name) {
                                Some(format!("__class_{}_get_{}", cn, prop_name))
                            } else {
                                None
                            }
                        })
                    })
                } else {
                    None
                };

                let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
                block = nb;
                let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("member access: object produced no value"))?;

                // arr.length  →  call ts_arr_len(obj)
                if member.property.name == "length" {
                    let obj_i64 = self.ensure_i64(obj, block)?;
                    let len: Value<'c, 'b> = block
                        .append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_len"),
                            &[obj_i64],
                            &[self.i64_type()],
                            self.loc,
                        ))
                        .result(0)?
                        .into();
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                    return Ok((Some(len), block));
                }

                let obj_i64 = self.ensure_i64(obj, block)?;

                // Getter dispatch
                if let Some(getter_name) = getter_mangled {
                    let val: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, &getter_name),
                        &[obj_i64],
                        &[self.i64_type()],
                        self.loc,
                    )).result(0)?.into();
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                    return Ok((Some(val), block));
                }

                let key_ptr = self.get_string_ptr(&prop_name, block)?;
                let val: Value<'c, 'b> = block
                    .append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                        &[obj_i64, key_ptr],
                        &[self.i64_type()],
                        self.loc,
                    ))
                    .result(0)?
                    .into();

                Ok((Some(val), block))
            }
            Expression::PrivateFieldExpression(priv_field) => {
                // #field → stored with mangled key "__priv_<name>"
                let field_key = format!("__priv_{}", priv_field.field.name.as_str());
                let (obj_opt, nb) = self.lower_expression(&priv_field.object, block, region, scope)?;
                block = nb;
                let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("private field: object produced no value"))?;
                let obj_i64 = self.ensure_i64(obj, block)?;
                let key_ptr = self.get_string_ptr(&field_key, block)?;
                let val: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                    &[obj_i64, key_ptr],
                    &[self.i64_type()],
                    self.loc,
                )).result(0)?.into();
                block.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[obj_i64], &[], self.loc,
                ));
                Ok((Some(val), block))
            }
            Expression::BinaryExpression(binop) => {
                self.lower_binary_expression(binop, block, region, scope)
            }
            Expression::LogicalExpression(logical) => {
                self.lower_logical_expression(logical, block, region, scope)
            }
            Expression::UnaryExpression(unary) => {
                self.lower_unary_expression(unary, block, region, scope)
            }
            Expression::UpdateExpression(update) => {
                self.lower_update_expression(update, block, region, scope)
            }
            Expression::AssignmentExpression(assign) => {
                self.lower_assignment_expression(assign, block, region, scope)
            }
            Expression::Identifier(ident) => {
                let name = ident.name.to_string();
                match scope.get(&name) {
                    Some(&v) => {
                        let v_i64 = self.ensure_i64(v, block)?;
                        block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"),
                            &[v_i64],
                            &[],
                            self.loc,
                        ));
                        Ok((Some(v), block))
                    }
                    None => bail!("undefined variable: {}", name),
                }
            }
            Expression::CallExpression(call) => {
                self.lower_call_expression(call, block, region, scope)
            }
            Expression::ConditionalExpression(cond) => {
                self.lower_conditional_expression(cond, block, region, scope)
            }
            Expression::NewExpression(new_expr) => {
                self.lower_new_expression(new_expr, block, region, scope)
            }
            Expression::ThisExpression(_) => {
                match scope.get("this") {
                    Some(&v) => {
                        // Follow the same ownership convention as Identifier:
                        // reading a variable produces an owned reference.
                        let v_i64 = self.ensure_i64(v, block)?;
                        block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"),
                            &[v_i64],
                            &[],
                            self.loc,
                        ));
                        Ok((Some(v), block))
                    }
                    None => bail!("'this' used outside of a class method"),
                }
            }
            Expression::AwaitExpression(aw) => {
                let (val_opt, nb) = self.lower_expression(&aw.argument, block, region, scope)?;
                block = nb;
                let val = val_opt.ok_or_else(|| anyhow::anyhow!("await: argument produced no value"))?;
                let val_i64 = self.ensure_i64(val, block)?;
                let result: Value<'c, 'b> = block
                    .append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_promise_await"),
                        &[val_i64],
                        &[self.i64_type()],
                        self.loc,
                    ))
                    .result(0)?.into();
                Ok((Some(result), block))
            }
            Expression::ParenthesizedExpression(pe) => {
                self.lower_expression(&pe.expression, block, region, scope)
            }
            _ => {
                tracing::debug!("skipping unimplemented expression kind");
                Ok((None, block))
            }
        }
    }

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

        // super.method(args) — dispatches to the parent class's method.
        if let Expression::StaticMemberExpression(member) = &call.callee {
            if let Expression::Super(_) = &member.object {
                let method_name = member.property.name.as_str().to_string();
                if let Some(class_name) = self.current_class.clone() {
                    if let Some(parent_name) = self.classes.get(&class_name)
                        .and_then(|s| s.parent.clone())
                    {
                        let mangled = format!("__class_{}_{}", parent_name, method_name);
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

        // console.log(x) → __ts_console_log_i32(x) or __ts_console_log_str(x)
        if let Expression::StaticMemberExpression(member) = &call.callee {
            if matches!(&member.object, Expression::Identifier(id) if id.name == "console")
                && member.property.name == "log"
            {
                if let Some(first_arg) = call.arguments.first() {
                    let expr = first_arg.as_expression()
                        .ok_or_else(|| anyhow::anyhow!("console.log: spread argument not supported"))?;
                    let (val_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    let val = val_opt
                        .ok_or_else(|| anyhow::anyhow!("console.log: argument produced no value"))?;
                    
                    let val_i64 = self.ensure_i64(val, block)?;
                    block.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "__ts_console_log_val"),
                        &[val_i64],
                        &[],
                        self.loc,
                    ));
                    
                    // ARC: console.log doesn't take ownership permanently, but it's a sink.
                    // Following our policy, we release the argument after the call.
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[val_i64], &[], self.loc));
                    
                    return Ok((None, block));
                }
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

        // User-defined function call
        if let Expression::Identifier(callee_id) = &call.callee {
            let name = callee_id.name.to_string();
            if let Some(sig) = self.funcs.get(&name).cloned() {
                // Lower arguments, coercing to match declared param types.
                let mut args: Vec<Value<'c, 'b>> = Vec::new();
                for (i, arg) in call.arguments.iter().enumerate() {
                    let expr = arg.as_expression()
                        .ok_or_else(|| anyhow::anyhow!("spread in function call not supported"))?;
                    let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    let v = v_opt
                        .ok_or_else(|| anyhow::anyhow!("argument produced no value"))?;
                    let expected = sig.param_types.get(i).copied().unwrap_or(self.i32_type());
                    let coerced = if expected == self.i64_type() {
                        self.ensure_i64(v, block)?
                    } else {
                        self.ensure_i32(v, block)?
                    };
                    args.push(coerced);
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
                        let obj = obj_opt
                            .ok_or_else(|| anyhow::anyhow!("method call: object produced no value"))?;
                        let obj_i64 = self.ensure_i64(obj, block)?;

                        // Lower arguments.
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

                        let result_types = &[self.i64_type()];
                        let op = block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, &mangled),
                            &args,
                            result_types,
                            self.loc,
                        ));
                        return Ok((Some(op.result(0)?.into()), block));
                    }
                }
            }
        }

        tracing::debug!("skipping unimplemented call expression");
        Ok((None, block))
    }

    // ── new Foo(args) ─────────────────────────────────────────────────────

    fn lower_new_expression<'b>(
        &mut self,
        new_expr: &NewExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let Expression::Identifier(callee_id) = &new_expr.callee else {
            bail!("new: only simple class names are supported as constructors");
        };
        let class_name = callee_id.name.to_string();

        let ctor_name = format!("__class_{}_constructor", class_name);
        let Some(sig) = self.funcs.get(&ctor_name).cloned() else {
            bail!("new {}: unknown class (constructor not found)", class_name);
        };

        let i64_type = self.i64_type();
        let mut args: Vec<Value<'c, 'b>> = Vec::new();
        for arg in &new_expr.arguments {
            let expr = arg.as_expression()
                .ok_or_else(|| anyhow::anyhow!("spread in new expression"))?;
            let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
            block = nb;
            let v = v_opt.ok_or_else(|| anyhow::anyhow!("new arg produced no value"))?;
            args.push(self.ensure_i64(v, block)?);
        }

        let result_types: Vec<melior::ir::Type<'c>> =
            sig.return_type.iter().copied().collect();

        let op = block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, &ctor_name),
            &args,
            &result_types,
            self.loc,
        ));

        Ok((Some(op.result(0)?.into()), block))
    }

}
