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
            Expression::StaticMemberExpression(member) => {
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
                    
                    // ARC: release obj after getting length.
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                    
                    return Ok((Some(len), block));
                }
                
                let obj_i64 = self.ensure_i64(obj, block)?;
                let key_ptr = self.get_string_ptr(&member.property.name, block)?;
                
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

        // User-defined function call
        if let Expression::Identifier(callee_id) = &call.callee {
            let name = callee_id.name.to_string();
            if let Some(sig) = self.funcs.get(&name).cloned() {
                // Lower arguments.
                let mut args: Vec<Value<'c, 'b>> = Vec::new();
                for arg in &call.arguments {
                    let expr = arg.as_expression()
                        .ok_or_else(|| anyhow::anyhow!("spread in function call not supported"))?;
                    let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
        block = nb;
        let v = v_opt
                        .ok_or_else(|| anyhow::anyhow!("argument produced no value"))?;
                    args.push(v);
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
