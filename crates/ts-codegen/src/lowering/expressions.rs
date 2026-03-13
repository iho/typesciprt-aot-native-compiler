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
                let v = num.value;
                // If it's an exact integer in i32 range, use the compact i32 representation.
                // Otherwise store as raw IEEE-754 f64 bits in i64.
                if v == v.trunc() && v >= i32::MIN as f64 && v <= i32::MAX as f64 {
                    Ok((Some(self.lower_numeric_literal(v as i64, block)?), block))
                } else {
                    let lit: Value<'c, 'b> = block.append_operation(arith::constant(
                        self.ctx,
                        IntegerAttribute::new(self.i64_type(), v.to_bits() as i64).into(),
                        self.loc,
                    )).result(0)?.into();
                    Ok((Some(lit), block))
                }
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
                // Math constants: Math.PI, Math.E, Math.LN2, …
                if let Expression::Identifier(obj_id) = &member.object {
                    if obj_id.name == "Math" {
                        let prop = member.property.name.as_str();
                        let f = match prop {
                            "PI"     => Some(std::f64::consts::PI),
                            "E"      => Some(std::f64::consts::E),
                            "LN2"    => Some(std::f64::consts::LN_2),
                            "LN10"   => Some(std::f64::consts::LN_10),
                            "LOG2E"  => Some(std::f64::consts::LOG2_E),
                            "LOG10E" => Some(std::f64::consts::LOG10_E),
                            "SQRT2"  => Some(std::f64::consts::SQRT_2),
                            _ => None,
                        };
                        if let Some(v) = f {
                            let lit: Value<'c, 'b> = block.append_operation(arith::constant(
                                self.ctx,
                                IntegerAttribute::new(self.i64_type(), v.to_bits() as i64).into(),
                                self.loc,
                            )).result(0)?.into();
                            return Ok((Some(lit), block));
                        }
                    }
                }

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

                // .size  →  ts_map_size(obj)  (for Map)
                if member.property.name == "size" {
                    let obj_i64 = self.ensure_i64(obj, block)?;
                    let sz: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_map_size"),
                        &[obj_i64], &[self.i64_type()], self.loc,
                    )).result(0)?.into();
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                    return Ok((Some(sz), block));
                }

                // .length  →  ts_val_length(obj)  (works for both arrays and strings)
                if member.property.name == "length" {
                    let obj_i64 = self.ensure_i64(obj, block)?;
                    let len: Value<'c, 'b> = block
                        .append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_val_length"),
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
                // `undefined` is a global that maps to the UNDEFINED NaN-box constant.
                if name == "undefined" {
                    let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                        self.ctx,
                        IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(),
                        self.loc,
                    )).result(0)?.into();
                    return Ok((Some(undef), block));
                }
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
                    None => {
                        // Check if this is a module-level global (not a function, not a builtin).
                        if self.module_global_names.contains(&name) {
                            let key_ptr = self.get_string_ptr(&name, block)?;
                            let val: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_get_module_global"),
                                &[key_ptr], &[self.i64_type()], self.loc,
                            )).result(0)?.into();
                            return Ok((Some(val), block));
                        }
                        // Check if this is a builtin function referenced as a first-class value.
                        // Resolve alias first (e.g. decodeURIComponent_ → decodeURIComponent).
                        let canonical = self.builtin_aliases.get(&name).cloned().unwrap_or_else(|| name.clone());
                        if let Some(wrapper_name) = self.ensure_builtin_wrapper(&canonical)? {
                            let i64t = self.i64_type();
                            let i32t = self.i32_type();
                            let (_, arity, _) = crate::lowering::Lowerer::builtin_wrapper_info(&canonical).unwrap();
                            let func_type_val = melior::ir::r#type::FunctionType::new(
                                self.ctx,
                                &vec![i64t; 1 + arity],
                                &[i64t],
                            ).into();
                            let ptr_type = melior::dialect::llvm::r#type::pointer(self.ctx, 0);
                            let fn_ref: Value<'c, 'b> = block.append_operation(
                                melior::ir::operation::OperationBuilder::new("func.constant", self.loc)
                                    .add_attributes(&[(
                                        melior::ir::Identifier::new(self.ctx, "value"),
                                        FlatSymbolRefAttribute::new(self.ctx, &wrapper_name).into(),
                                    )])
                                    .add_results(&[func_type_val])
                                    .build()?,
                            ).result(0)?.into();
                            let fn_ptr: Value<'c, 'b> = block.append_operation(
                                melior::ir::operation::OperationBuilder::new("builtin.unrealized_conversion_cast", self.loc)
                                    .add_operands(&[fn_ref])
                                    .add_results(&[ptr_type])
                                    .build()?,
                            ).result(0)?.into();
                            let arity_val: Value<'c, 'b> = block.append_operation(arith::constant(
                                self.ctx, IntegerAttribute::new(i32t, arity as i64).into(), self.loc,
                            )).result(0)?.into();
                            let fn_val: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_func_new"),
                                &[fn_ptr, arity_val], &[i64t], self.loc,
                            )).result(0)?.into();
                            return Ok((Some(fn_val), block));
                        }
                        // Check if this is a top-level function referenced as a first-class value.
                        if self.funcs.contains_key(&name) {
                            let i64t = self.i64_type();
                            let i32t = self.i32_type();
                            let sig = self.funcs[&name].clone();
                            let arity = sig.param_types.len() as i64;
                            let ptr_type = melior::dialect::llvm::r#type::pointer(self.ctx, 0);
                            let func_type_val = melior::ir::r#type::FunctionType::new(
                                self.ctx,
                                &sig.param_types,
                                &[i64t],
                            ).into();
                            let fn_ref: Value<'c, 'b> = block.append_operation(
                                melior::ir::operation::OperationBuilder::new("func.constant", self.loc)
                                    .add_attributes(&[(
                                        melior::ir::Identifier::new(self.ctx, "value"),
                                        FlatSymbolRefAttribute::new(self.ctx, &name).into(),
                                    )])
                                    .add_results(&[func_type_val])
                                    .build()?,
                            ).result(0)?.into();
                            let fn_ptr: Value<'c, 'b> = block.append_operation(
                                melior::ir::operation::OperationBuilder::new("builtin.unrealized_conversion_cast", self.loc)
                                    .add_operands(&[fn_ref])
                                    .add_results(&[ptr_type])
                                    .build()?,
                            ).result(0)?.into();
                            let arity_val: Value<'c, 'b> = block.append_operation(arith::constant(
                                self.ctx, IntegerAttribute::new(i32t, arity).into(), self.loc,
                            )).result(0)?.into();
                            let fn_val: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_func_new"),
                                &[fn_ptr, arity_val], &[i64t], self.loc,
                            )).result(0)?.into();
                            return Ok((Some(fn_val), block));
                        }
                        eprintln!("[debug] undefined var '{}', scope keys: {:?}", name, scope.keys().collect::<Vec<_>>());
                        bail!("undefined variable: {}", name)
                    }
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
            Expression::NullLiteral(_) => {
                // null → NaN-boxed NULL constant = NAN_MASK | TAG_NULL = 0x7FF9_0000_0000_0000
                let null_val: Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx,
                    IntegerAttribute::new(self.i64_type(), 0x7FF9_0000_0000_0000u64 as i64).into(),
                    self.loc,
                )).result(0)?.into();
                Ok((Some(null_val), block))
            }
            Expression::ChainExpression(chain) => {
                self.lower_chain_expression(chain, block, region, scope)
            }
            Expression::TemplateLiteral(tmpl) => {
                self.lower_template_literal(tmpl, block, region, scope)
            }
            Expression::ArrowFunctionExpression(arrow) => {
                self.lower_arrow_expr(arrow, block, region, scope)
            }
            Expression::FunctionExpression(func_expr) => {
                // Function expression: function(x) { ... } used as a value
                let params: Vec<&oxc_ast::ast::FormalParameter<'_>> =
                    func_expr.params.items.iter().collect();
                let rest_name = func_expr.params.rest.as_ref()
                    .and_then(|r| if let BindingPattern::BindingIdentifier(id) = &r.rest.argument { Some(id.name.as_str()) } else { None });
                let body = func_expr.body.as_deref();
                match self.lower_arrow_like(&params, rest_name, body, None, block, region, scope) {
                    Ok((fn_val, nb)) => Ok((Some(fn_val), nb)),
                    Err(e) => Err(e),
                }
            }
            Expression::RegExpLiteral(re_lit) => {
                // /pattern/flags → ts_regexp_new(source_ptr, flags_ptr)
                let source = re_lit.regex.pattern.text.as_str().to_string();
                let flags = re_lit.regex.flags.to_string();
                let src_ptr = self.get_string_ptr(&source, block)?;
                let flags_ptr = self.get_string_ptr(&flags, block)?;
                let i64t = self.i64_type();
                let re_val: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_regexp_new"),
                    &[src_ptr, flags_ptr], &[i64t], self.loc,
                )).result(0)?.into();
                Ok((Some(re_val), block))
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
                // Lower args
                let mut args = vec![obj_i64];
                for arg in &call.arguments {
                    let expr = arg.as_expression()
                        .ok_or_else(|| anyhow::anyhow!("spread in private method call"))?;
                    let (v_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    let v = v_opt.ok_or_else(|| anyhow::anyhow!("private method arg produced no value"))?;
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

        // console.log(a, b, …) → print each arg space-separated, then newline
        if let Expression::StaticMemberExpression(member) = &call.callee {
            if matches!(&member.object, Expression::Identifier(id) if id.name == "console")
                && member.property.name == "log"
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
                | "encodeURIComponent" | "decodeURIComponent" | "encodeURI" | "decodeURI" => {
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

        // Dynamic function call: callee is an identifier holding a TsFunction (i64) in scope
        if let Expression::Identifier(callee_id) = &call.callee {
            let name = callee_id.name.as_str();
            let in_scope_as_i64 = scope.get(name)
                .map(|v| v.r#type() == self.i64_type())
                .unwrap_or(false);
            if in_scope_as_i64 && !self.funcs.contains_key(name) {
                let fn_val = scope[name];
                return self.lower_dynamic_call(fn_val, &call.arguments, block, region, scope);
            }
        }

        // User-defined function call
        if let Expression::Identifier(callee_id) = &call.callee {
            let raw_name = callee_id.name.to_string();
            // Resolve alias: `const foo = bar` — use the canonical name for the MLIR call.
            let name = if let Some(canon) = self.builtin_aliases.get(&raw_name) {
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
                let i32t = self.i32_type();
                let i64t2 = self.i64_type();
                for arg in call.arguments.iter() {
                    match arg {
                        oxc_ast::ast::Argument::SpreadElement(spread) => {
                            // Evaluate spread expression (should be TsArray), flatten into all_arg_vals.
                            let (v_opt, nb) = self.lower_expression(&spread.argument, block, region, scope)?;
                            block = nb;
                            let arr = v_opt.ok_or_else(|| anyhow::anyhow!("spread arg produced no value"))?;
                            let arr_i64 = self.ensure_i64(arr, block)?;
                            // Get array length at compile time is not possible, use runtime loop approach:
                            // We don't have a loop in codegen here, so we emit ts_arr_get for indices 0..max
                            // and build a dynamic approach. Simplest: use ts_arr_len then push individually.
                            // For most cases (known-arity functions), the spread array has a fixed size.
                            // Emit a runtime-length iteration: generate up to 8 elements.
                            let len_val: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_length"),
                                &[arr_i64], &[i64t2], self.loc,
                            )).result(0)?.into();
                            let len_i32 = self.ensure_i32(len_val, block)?;
                            // We'll unroll up to the needed number of params.
                            // Just emit gets for 0..(n_regular - current_count) clamped at 8.
                            let needed = n_regular.saturating_sub(all_arg_vals.len()).min(8);
                            for idx in 0..needed {
                                let idx_c: Value<'c, 'b> = block.append_operation(arith::constant(
                                    self.ctx, IntegerAttribute::new(i32t, idx as i64).into(), self.loc,
                                )).result(0)?.into();
                                // Check idx < len
                                let in_bounds: Value<'c, 'b> = block.append_operation(arith::cmpi(
                                    self.ctx, arith::CmpiPredicate::Slt, idx_c, len_i32, self.loc,
                                )).result(0)?.into();
                                // Get element (or undefined if out of bounds)
                                let elem: Value<'c, 'b> = block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
                                    &[arr_i64, idx_c], &[i64t2], self.loc,
                                )).result(0)?.into();
                                let undef_c: Value<'c, 'b> = block.append_operation(arith::constant(
                                    self.ctx, IntegerAttribute::new(i64t2, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                                )).result(0)?.into();
                                // Select: in_bounds ? elem : undefined
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
                if ns == "Math" || ns == "Object" || ns == "Array" || ns == "String" || ns == "JSON" || ns == "Promise" {
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
                        ("Math", "abs")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_abs"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "floor") => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_floor"), &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "ceil")  => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_ceil"),  &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "round") => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_round"), &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "sqrt")  => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_sqrt"),  &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "trunc") => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_trunc"), &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "log")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_log"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "log2")  => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_log2"),  &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "log10") => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_log10"), &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "sin")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_sin"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "cos")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_cos"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "tan")   => Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_tan"),   &[*arg_vals.first().unwrap_or(&undef_i64)], &[i64t], self.loc)).result(0)?.into()),
                        ("Math", "min")   => { let a = *arg_vals.first().unwrap_or(&undef_i64); let b = *arg_vals.get(1).unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_min"), &[a, b], &[i64t], self.loc)).result(0)?.into()) }
                        ("Math", "max")   => { let a = *arg_vals.first().unwrap_or(&undef_i64); let b = *arg_vals.get(1).unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_max"), &[a, b], &[i64t], self.loc)).result(0)?.into()) }
                        ("Math", "pow")   => { let a = *arg_vals.first().unwrap_or(&undef_i64); let b = *arg_vals.get(1).unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_pow"), &[a, b], &[i64t], self.loc)).result(0)?.into()) }
                        ("Math", "atan2") => { let a = *arg_vals.first().unwrap_or(&undef_i64); let b = *arg_vals.get(1).unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_atan2"), &[a, b], &[i64t], self.loc)).result(0)?.into()) }
                        ("Math", "hypot") => { let a = *arg_vals.first().unwrap_or(&undef_i64); let b = *arg_vals.get(1).unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_math_hypot"), &[a, b], &[i64t], self.loc)).result(0)?.into()) }
                        // ── Object ────────────────────────────────────────────
                        ("Object", "keys")         => { let o = *arg_vals.first().unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_keys"),         &[o], &[i64t], self.loc)).result(0)?.into()) }
                        ("Object", "values")       => { let o = *arg_vals.first().unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_values"),       &[o], &[i64t], self.loc)).result(0)?.into()) }
                        ("Object", "entries")      => { let o = *arg_vals.first().unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_entries"),      &[o], &[i64t], self.loc)).result(0)?.into()) }
                        ("Object", "assign")       => { let t = *arg_vals.first().unwrap_or(&undef_i64); let s = *arg_vals.get(1).unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_assign"),       &[t, s], &[i64t], self.loc)).result(0)?.into()) }
                        ("Object", "create")       => { let p = *arg_vals.first().unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_create"),       &[p], &[i64t], self.loc)).result(0)?.into()) }
                        ("Object", "fromEntries")  => { let a = *arg_vals.first().unwrap_or(&undef_i64); Some(block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_from_entries"), &[a], &[i64t], self.loc)).result(0)?.into()) }
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
                                 "sin"|"cos"|"tan"|"min"|"max"|"pow"|"atan2"|"hypot") |
                        ("Object", "keys"|"values"|"entries"|"assign"|"create"|"fromEntries") |
                        ("Array", "isArray") |
                        ("String", "fromCharCode") |
                        ("JSON", "stringify"|"parse") |
                        ("Promise", "resolve"|"reject"|"all")
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

        // ── Built-in array / string method dispatch ──────────────────────────
        if let Expression::StaticMemberExpression(member) = &call.callee {
            let method_name = member.property.name.as_str().to_string();
            let is_builtin = matches!(method_name.as_str(),
                "push" | "pop" | "indexOf" | "includes" | "join" |
                "slice" | "toUpperCase" | "toLowerCase" | "trim" | "split" |
                // Array HOFs
                "map" | "filter" | "forEach" | "reduce" | "find" |
                "findIndex" | "some" | "every" | "sort" | "flatMap" | "flat" |
                // String methods
                "replace" | "replaceAll" | "startsWith" | "endsWith" |
                "padStart" | "padEnd" | "charAt" | "charCodeAt" | "repeat" |
                // Map methods
                "set" | "get" | "has" | "delete" | "clear" | "keys" | "values" | "entries" |
                // RegExp methods
                "test" | "exec" |
                // String match/replace with RegExp
                "match" |
                // Request/Response body
                "text" | "json" |
                // URLSearchParams
                "toString" | "getAll"
            );
            if is_builtin {
                // Evaluate receiver
                let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
                block = nb;
                let obj = obj_opt
                    .ok_or_else(|| anyhow::anyhow!("builtin method: object produced no value"))?;
                let obj_i64 = self.ensure_i64(obj, block)?;

                // Evaluate all arguments
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
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push"),
                            &[obj_i64, val], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "pop" => {
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_pop"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "indexOf" => {
                        let search = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_val_index_of"),
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
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_for_each"),
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
                    "flatMap" => {
                        let cb = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_flat_map"),
                            &[obj_i64, cb], &[i64t], self.loc,
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
                    // Map methods
                    "set" => {
                        let key = arg_vals.first().copied().unwrap_or(undefined_i64);
                        let val = arg_vals.get(1).copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_map_set"),
                            &[obj_i64, key, val], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "get" => {
                        let key = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_map_get"),
                            &[obj_i64, key], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "has" => {
                        let key = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_map_has"),
                            &[obj_i64, key], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "delete" => {
                        let key = arg_vals.first().copied().unwrap_or(undefined_i64);
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_map_delete"),
                            &[obj_i64, key], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "clear" => {
                        block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_map_clear"),
                            &[obj_i64], &[], self.loc,
                        ));
                        None
                    }
                    "keys" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_map_keys"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "values" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_map_values"),
                            &[obj_i64], &[i64t], self.loc,
                        )).result(0)?.into())
                    }
                    "entries" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_map_entries"),
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
                    // ── URLSearchParams methods ───────────────────────────────
                    "toString" => {
                        Some(block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_urlsearchparams_to_string"),
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
            }
        }

        // Generic fallback: evaluate callee as a dynamic function value and dispatch.
        let (fn_opt, nb) = self.lower_expression(&call.callee, block, region, scope)?;
        block = nb;
        if let Some(fn_val) = fn_opt {
            return self.lower_dynamic_call(fn_val, &call.arguments, block, region, scope);
        }

        tracing::debug!("skipping unimplemented call expression");
        Ok((None, block))
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
                                    &[args_arr, v_i64], &[i64t], self.loc,
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

    // ── Chain expressions (optional chaining: obj?.prop / obj?.[idx]) ────

    fn lower_chain_expression<'b>(
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
                self.lower_call_expression(call, block, region, scope)
            }
            _ => {
                tracing::debug!("skipping unimplemented chain element");
                Ok((None, block))
            }
        }
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
            // Non-optional: use the existing computed member handler.
            let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
            block = nb;
            let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("chain computed: object produced no value"))?;
            let obj_i64 = self.ensure_i64(obj, block)?;
            let (idx_opt, nb) = self.lower_expression(&member.expression, block, region, scope)?;
            block = nb;
            let idx = idx_opt.ok_or_else(|| anyhow::anyhow!("chain computed: index produced no value"))?;
            let idx_i32 = self.ensure_i32(idx, block)?;
            let val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
                &[obj_i64, idx_i32],
                &[self.i64_type()],
                self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[obj_i64], &[], self.loc,
            ));
            let idx_i64 = self.ensure_i64(idx, block)?;
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[idx_i64], &[], self.loc,
            ));
            return Ok((Some(val), block));
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

        // Re-evaluate the index in the access block.
        let (idx_opt, nb) = self.lower_expression(&member.expression, access_block, region, scope)?;
        let access_block_after_idx = nb;
        let idx = idx_opt.ok_or_else(|| anyhow::anyhow!("optional computed: index produced no value"))?;
        let idx_i32 = self.ensure_i32(idx, access_block_after_idx)?;
        let result: Value<'c, 'b> = access_block_after_idx.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
            &[obj_i64, idx_i32],
            &[self.i64_type()],
            self.loc,
        )).result(0)?.into();
        access_block_after_idx.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[obj_i64], &[], self.loc,
        ));
        let idx_i64 = self.ensure_i64(idx, access_block_after_idx)?;
        access_block_after_idx.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[idx_i64], &[], self.loc,
        ));
        access_block_after_idx.append_operation(cf::br(&merge_block, &[result], self.loc));

        let merged: Value<'c, 'b> = merge_block.argument(0)?.into();
        Ok((Some(merged), merge_block))
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
            let map_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_map_new"),
                &[], &[i64t], self.loc,
            )).result(0)?.into();
            // If initial iterable arg provided (e.g. new Map([[k,v]])), populate it
            if !new_expr.arguments.is_empty() {
                if let Some(expr) = new_expr.arguments[0].as_expression() {
                    let (arr_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    if let Some(arr) = arr_opt {
                        let arr_i64 = self.ensure_i64(arr, block)?;
                        // Iterate pairs
                        // Simple: call ts_map_from_arr if available, or do inline loop
                        // For now just skip population (no ts_map_from_arr yet)
                        let _ = arr_i64;
                    }
                }
            }
            return Ok((Some(map_val), block));
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

    // ── Template literals ─────────────────────────────────────────────────
    // Lower `` `Hello ${name}!` `` by concatenating quasis and coerced expressions.

    pub(super) fn lower_template_literal<'b>(
        &mut self,
        tmpl: &TemplateLiteral<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let i64t = self.i64_type();

        // Start with the first quasi segment as a string TsVal.
        let first_cooked = tmpl.quasis.first()
            .and_then(|q| q.value.cooked.as_deref())
            .unwrap_or("");
        let mut acc: Value<'c, 'b> = self.lower_string_literal(first_cooked, block)?;

        // Interleave expressions and subsequent quasis.
        for (i, expr) in tmpl.expressions.iter().enumerate() {
            // Convert expression to string.
            let (val_opt, nb) = self.lower_expression(expr, block, region, scope)?;
            block = nb;
            let val = val_opt.ok_or_else(|| anyhow::anyhow!("template literal: expression produced no value"))?;
            let val_i64 = self.ensure_i64(val, block)?;
            let expr_str: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_val_to_string"),
                &[val_i64],
                &[i64t],
                self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[val_i64], &[], self.loc,
            ));

            // Concat acc + expr_str → new_acc; release both operands.
            let after_expr: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_string_concat"),
                &[acc, expr_str],
                &[i64t],
                self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[acc], &[], self.loc,
            ));
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[expr_str], &[], self.loc,
            ));
            acc = after_expr;

            // Append the following quasi segment (if non-empty).
            let next_cooked = tmpl.quasis.get(i + 1)
                .and_then(|q| q.value.cooked.as_deref())
                .unwrap_or("");
            if !next_cooked.is_empty() {
                let quasi_str: Value<'c, 'b> = self.lower_string_literal(next_cooked, block)?;
                let merged: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_string_concat"),
                    &[acc, quasi_str],
                    &[i64t],
                    self.loc,
                )).result(0)?.into();
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[acc], &[], self.loc,
                ));
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[quasi_str], &[], self.loc,
                ));
                acc = merged;
            }
        }

        Ok((Some(acc), block))
    }

    // ── Arrow function expressions ─────────────────────────────────────────

    pub(super) fn lower_arrow_expr<'b>(
        &mut self,
        arrow: &ArrowFunctionExpression<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let params: Vec<&oxc_ast::ast::FormalParameter<'_>> =
            arrow.params.items.iter().collect();
        let rest_name = arrow.params.rest.as_ref()
            .and_then(|r| if let BindingPattern::BindingIdentifier(id) = &r.rest.argument { Some(id.name.as_str()) } else { None });
        let (fn_val, nb) = self.lower_arrow_like(&params, rest_name, Some(&arrow.body), None, block, region, scope)?;
        Ok((Some(fn_val), nb))
    }

    pub fn lower_arrow_like<'b>(
        &mut self,
        params: &[&oxc_ast::ast::FormalParameter<'_>],
        rest_param_name: Option<&str>,
        block_body: Option<&oxc_ast::ast::FunctionBody<'_>>,
        expr_body: Option<&oxc_ast::ast::FunctionBody<'_>>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        outer_scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Value<'c, 'b>, BlockRef<'c, 'b>)> {
        let i64_type = self.i64_type();
        let ptr_type = self.llvm_ptr_type();
        let i32_type = self.i32_type();

        let n = self.arrow_count;
        self.arrow_count += 1;
        let name = format!("__arrow_{}", n);
        let has_rest = rest_param_name.is_some();
        // arity = number of regular params; if has_rest, one extra MLIR param for the rest array
        let arity = params.len() + if has_rest { 1 } else { 0 };

        // ── Free-variable analysis ───────────────────────────────────────────
        // Collect names of outer-scope variables referenced in the body (not including params).
        let mut param_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        for p in params.iter() {
            collect_locals_binding(&p.pattern, &mut param_names);
        }
        if let Some(rn) = rest_param_name {
            param_names.insert(rn.to_string());
        }

        // outer_keys = scope variables + all known function names (so named functions are not captured)
        // Also include well-known globals so they are not treated as free variables.
        let mut outer_keys: std::collections::HashSet<String> = outer_scope.keys().cloned().collect();
        for k in self.funcs.keys() { outer_keys.insert(k.clone()); }
        for builtin in &[
            "undefined", "null", "true", "false",
            "parseInt", "parseFloat", "isNaN", "isFinite", "Number", "String", "Boolean",
            "encodeURIComponent", "decodeURIComponent", "encodeURI", "decodeURI",
            "JSON", "Math", "Object", "Array", "Promise", "Error",
            "TypeError", "RangeError", "SyntaxError", "ReferenceError",
            "console", "setTimeout", "clearTimeout", "setInterval", "clearInterval",
            "fetch", "URL", "URLSearchParams", "Headers", "Request", "Response",
            "Symbol", "Map", "Set", "WeakMap", "WeakRef", "Reflect", "Proxy",
            "serve", "sleep", "select", "addEventListener", "removeEventListener",
            "queueMicrotask", "structuredClone", "crypto", "performance",
        ] {
            outer_keys.insert(builtin.to_string());
        }
        // Add names from builtin_aliases so they're not captured either.
        for k in self.builtin_aliases.keys() { outer_keys.insert(k.clone()); }
        // Module globals are accessed via ts_get_module_global at runtime, not captured.
        for k in self.module_global_names.iter() { outer_keys.insert(k.clone()); }

        let body_stmts: &[oxc_ast::ast::Statement<'_>] = block_body
            .or(expr_body)
            .map(|b| b.statements.as_slice())
            .unwrap_or(&[]);

        let mut free_vars: Vec<String> = Vec::new();
        collect_free_vars_stmts(body_stmts, &param_names, &outer_keys, &mut free_vars);
        // Deduplicate preserving order.
        {
            let mut seen = std::collections::HashSet::new();
            free_vars.retain(|v| seen.insert(v.clone()));
        }
        let has_captures = !free_vars.is_empty();

        // If there are captures, the MLIR function has (env, param0, ...) else (param0, ...).
        let total_mlir_params = if has_captures { 1 + arity } else { arity };
        let param_specs: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            (0..total_mlir_params).map(|_| (i64_type, self.loc)).collect();
        let arrow_region = Region::new();
        let arrow_entry = arrow_region.append_block(Block::new(&param_specs));

        let mut arrow_scope: HashMap<String, Value<'_, '_>> = HashMap::new();

        // If closure, bind captured variables by extracting from env array (arg 0).
        if has_captures {
            let env_arg: Value<'_, '_> = arrow_entry.argument(0)?.into();
            // Store env under a reserved key so assignments can write back mutations.
            arrow_scope.insert("__env".to_string(), env_arg);
            for (idx, var_name) in free_vars.iter().enumerate() {
                let idx_val: Value<'_, '_> = arrow_entry.append_operation(arith::constant(
                    self.ctx,
                    IntegerAttribute::new(i32_type, idx as i64).into(),
                    self.loc,
                )).result(0)?.into();
                let captured: Value<'_, '_> = arrow_entry.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
                    &[env_arg, idx_val],
                    &[i64_type],
                    self.loc,
                )).result(0)?.into();
                arrow_scope.insert(var_name.clone(), captured);
            }
        }

        // Bind params (offset by 1 if closure).
        let param_offset = if has_captures { 1 } else { 0 };
        for (i, param) in params.iter().enumerate() {
            let arg_val: Value<'_, '_> = arrow_entry.argument(param_offset + i)?.into();
            match &param.pattern {
                BindingPattern::BindingIdentifier(id) => {
                    arrow_scope.insert(id.name.to_string(), arg_val);
                }
                BindingPattern::ArrayPattern(arr_pat) => {
                    // Parameter is destructured: e.g. ([key, value]) or ([[, route]]) => {...}
                    let arg_i64 = self.ensure_i64(arg_val, arrow_entry)?;
                    self.destructure_array_pattern_into_scope(arr_pat, arg_i64, arrow_entry, &mut arrow_scope)?;
                }
                BindingPattern::ObjectPattern(obj_pat) => {
                    // Parameter is destructured: e.g. ({key, value}) => {...}
                    let arg_i64 = self.ensure_i64(arg_val, arrow_entry)?;
                    for prop in &obj_pat.properties {
                        if let (oxc_ast::ast::PropertyKey::StaticIdentifier(key_id),
                                BindingPattern::BindingIdentifier(val_id)) =
                            (&prop.key, &prop.value)
                        {
                            let key_ptr = self.get_string_ptr(key_id.name.as_str(), arrow_entry)?;
                            let prop_val: Value<'_, '_> = arrow_entry.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                                &[arg_i64, key_ptr], &[i64_type], self.loc,
                            )).result(0)?.into();
                            arrow_scope.insert(val_id.name.to_string(), prop_val);
                        }
                    }
                }
                _ => {}
            }
        }
        // Bind rest param (last MLIR param after regular params).
        if let Some(rn) = rest_param_name {
            let rest_idx = param_offset + params.len();
            let rest_val: Value<'_, '_> = arrow_entry.argument(rest_idx)?.into();
            arrow_scope.insert(rn.to_string(), rest_val);
        }

        let saved_return_type = self.fn_return_type;
        let saved_is_async = self.is_async;
        let saved_env_indices = std::mem::replace(
            &mut self.closure_env_indices,
            if has_captures {
                free_vars.iter().cloned().enumerate().map(|(i, v)| (v, i)).collect()
            } else {
                HashMap::new()
            },
        );
        self.fn_return_type = i64_type;
        self.is_async = false;

        let mut current_block = arrow_entry;

        // Emit default parameter checks: if param === undefined, use initializer.
        for (i, param) in params.iter().enumerate() {
            let Some(init_expr) = &param.initializer else { continue };
            let BindingPattern::BindingIdentifier(id) = &param.pattern else { continue };
            let param_name = id.name.to_string();

            let param_val = arrow_entry.argument(i)?.into();
            let param_i64 = self.ensure_i64(param_val, current_block)?;

            let is_undef: Value<'_, '_> = current_block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_undefined"),
                &[param_i64], &[self.i32_type()], self.loc,
            )).result(0)?.into();
            let is_undef_i1 = self.ensure_i1(is_undef, current_block)?;

            // merge_block: receives (final_param_i64: i64)
            let merge_block = arrow_region.append_block(Block::new(&[(i64_type, self.loc)]));
            let default_block = arrow_region.append_block(Block::new(&[]));

            // if undefined → default_block; else → merge_block with raw param value
            current_block.append_operation(cf::cond_br(
                self.ctx, is_undef_i1,
                &default_block, &merge_block,
                &[], &[param_i64],
                self.loc,
            ));

            // default_block: evaluate initializer, jump to merge
            let mut default_scope = arrow_scope.clone();
            let (init_val_opt, post_init_block) =
                self.lower_expression(init_expr, default_block, &arrow_region, &mut default_scope)?;
            let init_val = init_val_opt.ok_or_else(|| anyhow::anyhow!("default param '{}': initializer produced no value", param_name))?;
            let init_i64 = self.ensure_i64(init_val, post_init_block)?;
            post_init_block.append_operation(cf::br(&merge_block, &[init_i64], self.loc));

            // Update scope with the resolved param value from merge
            let final_param: Value<'_, '_> = merge_block.argument(0)?.into();
            arrow_scope.insert(param_name, final_param);
            current_block = merge_block;
        }

        let mut result_val: Option<Value<'_, '_>> = None;

        let body_opt = block_body.or(expr_body);
        if let Some(body) = body_opt {
            // Pre-seed ALL local bindings (vars + inner function names) as `undefined` so
            // they appear in scope for phi-node tracking and free-var analysis.
            let undef_placeholder: Value<'_, '_> = current_block.append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(i64_type, 0x7FF8_0000_0000_0000u64 as i64).into(),
                self.loc,
            )).result(0)?.into();
            for stmt in &body.statements {
                if let oxc_ast::ast::Statement::VariableDeclaration(vd) = stmt {
                    for decl in &vd.declarations {
                        predeclare_binding(&decl.id, undef_placeholder, &mut arrow_scope);
                    }
                } else if let oxc_ast::ast::Statement::FunctionDeclaration(inner_fn) = stmt {
                    if let Some(fn_id) = &inner_fn.id {
                        let fn_name = fn_id.name.to_string();
                        if !arrow_scope.contains_key(&fn_name) {
                            arrow_scope.insert(fn_name, undef_placeholder);
                        }
                    }
                }
            }

            // Process all statements in source order. FunctionDeclarations are handled inline
            // (closure created at their declaration position, not truly hoisted), which is
            // correct for the common case where inner functions are declared before they're called.
            for stmt in &body.statements {
                if let oxc_ast::ast::Statement::FunctionDeclaration(inner_fn) = stmt {
                    let Some(fn_id) = &inner_fn.id else { continue };
                    let fn_name = fn_id.name.to_string();
                    let inner_params: Vec<&oxc_ast::ast::FormalParameter<'_>> =
                        inner_fn.params.items.iter().collect();
                    let inner_body = inner_fn.body.as_deref();
                    let inner_rest = inner_fn.params.rest.as_ref()
                        .and_then(|r| if let BindingPattern::BindingIdentifier(id) = &r.rest.argument { Some(id.name.as_str()) } else { None });
                    let saved_async = self.is_async;
                    self.is_async = inner_fn.r#async;
                    let (fn_val, nb) = self.lower_arrow_like(
                        &inner_params,
                        inner_rest,
                        inner_body,
                        None,
                        current_block,
                        &arrow_region,
                        &mut arrow_scope,
                    )?;
                    self.is_async = saved_async;
                    current_block = nb;

                    // Fix up self-reference: if fn_name is captured by the closure (recursive),
                    // the env slot was set to undefined (pre-seed). Patch it with the actual closure.
                    {
                        let mut inner_outer_keys: NameSet = arrow_scope.keys().cloned().collect();
                        let mut inner_param_set: NameSet = NameSet::new();
                        for p in &inner_fn.params.items {
                            if let BindingPattern::BindingIdentifier(id) = &p.pattern {
                                inner_param_set.insert(id.name.to_string());
                            }
                        }
                        inner_outer_keys.insert(fn_name.clone());
                        let mut inner_free_vars: Vec<String> = Vec::new();
                        if let Some(inner_body_ref) = inner_fn.body.as_deref() {
                            collect_free_vars_stmts(
                                &inner_body_ref.statements,
                                &inner_param_set,
                                &inner_outer_keys,
                                &mut inner_free_vars,
                            );
                            let mut seen = std::collections::HashSet::new();
                            inner_free_vars.retain(|v| seen.insert(v.clone()));
                        }
                        if let Some(self_idx) = inner_free_vars.iter().position(|v| v == &fn_name) {
                            let env_arr = current_block.append_operation(func::call(
                                self.ctx,
                                FlatSymbolRefAttribute::new(self.ctx, "ts_closure_get_env"),
                                &[fn_val], &[i64_type], self.loc,
                            )).result(0)?.into();
                            let self_idx_val = current_block.append_operation(
                                arith::constant(self.ctx,
                                    IntegerAttribute::new(self.i32_type(), self_idx as i64).into(),
                                    self.loc,
                                )
                            ).result(0)?.into();
                            current_block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_set"),
                                &[env_arr, self_idx_val, fn_val], &[], self.loc,
                            ));
                            current_block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                &[env_arr], &[], self.loc,
                            ));
                        }
                    }

                    arrow_scope.insert(fn_name, fn_val);
                } else {
                    let (v, nb) = self.lower_statement(stmt, current_block, &arrow_region, &mut arrow_scope, &[])?;
                    current_block = nb;
                    if let Some(val) = v { result_val = Some(val); }
                }
            }
        }

        self.fn_return_type = saved_return_type;
        self.is_async = saved_is_async;
        self.closure_env_indices = saved_env_indices;

        // Default return: UNDEFINED
        let default_undef: Value<'_, '_> = current_block.append_operation(arith::constant(
            self.ctx,
            IntegerAttribute::new(i64_type, 0x7FF8_0000_0000_0000u64 as i64).into(),
            self.loc,
        )).result(0)?.into();
        let ret = result_val.unwrap_or(default_undef);
        let ret_i64 = self.ensure_i64(ret, current_block)?;
        if current_block.terminator().is_none() {
            current_block.append_operation(func::r#return(&[ret_i64], self.loc));
        }

        let func_type = FunctionType::new(self.ctx, &vec![i64_type; total_mlir_params], &[i64_type]);
        let op = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, &name),
            TypeAttribute::new(func_type.into()),
            arrow_region,
            &[],
            self.loc,
        );
        self.module.body().append_operation(op);

        // Register so dynamic calls know the signature.
        self.funcs.insert(name.clone(), FuncSig {
            param_types: vec![i64_type; total_mlir_params],
            return_type: Some(i64_type),
            has_rest: false,
        });

        // Get a function reference via func.constant, then cast to !llvm.ptr.
        let func_type_val: melior::ir::Type<'c> = FunctionType::new(
            self.ctx, &vec![i64_type; total_mlir_params], &[i64_type],
        ).into();
        let fn_ref: Value<'c, 'b> = block.append_operation(
            OperationBuilder::new("func.constant", self.loc)
                .add_attributes(&[(
                    Identifier::new(self.ctx, "value"),
                    FlatSymbolRefAttribute::new(self.ctx, &name).into(),
                )])
                .add_results(&[func_type_val])
                .build()?,
        ).result(0)?.into();
        let fn_ptr: Value<'c, 'b> = block.append_operation(
            OperationBuilder::new("builtin.unrealized_conversion_cast", self.loc)
                .add_operands(&[fn_ref])
                .add_results(&[ptr_type])
                .build()?,
        ).result(0)?.into();

        let arity_val: Value<'c, 'b> = block.append_operation(arith::constant(
            self.ctx,
            IntegerAttribute::new(i32_type, arity as i64).into(),
            self.loc,
        )).result(0)?.into();

        let fn_val: Value<'c, 'b> = if has_captures {
            // Build env array from current values of captured variables.
            let env_arr: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
                &[block.append_operation(arith::constant(
                    self.ctx,
                    IntegerAttribute::new(i32_type, free_vars.len() as i64).into(),
                    self.loc,
                )).result(0)?.into()],
                &[i64_type],
                self.loc,
            )).result(0)?.into();

            for (idx, var_name) in free_vars.iter().enumerate() {
                let captured_val = if let Some(&v) = outer_scope.get(var_name) {
                    let v_i64 = self.ensure_i64(v, block)?;
                    // Retain: env array takes ownership
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"),
                        &[v_i64], &[], self.loc,
                    ));
                    v_i64
                } else {
                    // Not found in scope: use undefined
                    block.append_operation(arith::constant(
                        self.ctx,
                        IntegerAttribute::new(i64_type, 0x7FF8_0000_0000_0000u64 as i64).into(),
                        self.loc,
                    )).result(0)?.into()
                };
                let idx_i32: Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx,
                    IntegerAttribute::new(i32_type, idx as i64).into(),
                    self.loc,
                )).result(0)?.into();
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_set"),
                    &[env_arr, idx_i32, captured_val], &[], self.loc,
                ));
            }

            let result = block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_closure_new"),
                &[fn_ptr, arity_val, env_arr],
                &[i64_type],
                self.loc,
            )).result(0)?.into();
            // ts_closure_new retains env; release our temporary ref
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[env_arr], &[], self.loc,
            ));
            result
        } else {
            block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_func_new"),
                &[fn_ptr, arity_val],
                &[i64_type],
                self.loc,
            )).result(0)?.into()
        };

        Ok((fn_val, block))
    }

    /// Recursively destructure an ArrayPattern into scope. Handles nested patterns.
    pub(super) fn destructure_array_pattern_into_scope<'b>(
        &mut self,
        arr_pat: &oxc_ast::ast::ArrayPattern<'_>,
        arr_val: Value<'c, 'b>,
        block: BlockRef<'c, 'b>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<()> {
        let i64t = self.i64_type();
        let i32t = self.i32_type();
        for (elem_idx, elem) in arr_pat.elements.iter().enumerate() {
            let Some(elem_pat) = elem else { continue }; // skip holes/elisions
            let idx_c: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i32t, elem_idx as i64).into(), self.loc,
            )).result(0)?.into();
            let elem_val: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
                &[arr_val, idx_c], &[i64t], self.loc,
            )).result(0)?.into();
            match elem_pat {
                BindingPattern::BindingIdentifier(id) => {
                    scope.insert(id.name.to_string(), elem_val);
                }
                BindingPattern::ArrayPattern(inner) => {
                    self.destructure_array_pattern_into_scope(inner, elem_val, block, scope)?;
                }
                BindingPattern::ObjectPattern(obj_pat) => {
                    for prop in &obj_pat.properties {
                        if let (oxc_ast::ast::PropertyKey::StaticIdentifier(key_id),
                                BindingPattern::BindingIdentifier(val_id)) =
                            (&prop.key, &prop.value)
                        {
                            let key_ptr = self.get_string_ptr(key_id.name.as_str(), block)?;
                            let prop_val: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                                &[elem_val, key_ptr], &[i64t], self.loc,
                            )).result(0)?.into();
                            scope.insert(val_id.name.to_string(), prop_val);
                        }
                    }
                }
                BindingPattern::AssignmentPattern(ap) => {
                    if let BindingPattern::BindingIdentifier(id) = &ap.left {
                        scope.insert(id.name.to_string(), elem_val); // TODO: apply default
                    }
                }
            }
        }
        Ok(())
    }

    /// Call a built-in function by its canonical JS name (e.g. "decodeURIComponent").
    /// Returns `Some(result)` if it's a known built-in, `None` otherwise.
    pub(super) fn call_builtin_by_name<'b>(
        &mut self,
        name: &str,
        arg_vals: &[Value<'c, 'b>],
        undef_i64: Value<'c, 'b>,
        block: BlockRef<'c, 'b>,
    ) -> Result<Option<Value<'c, 'b>>> {
        let i64t = self.i64_type();
        let result = match name {
            "decodeURIComponent" => {
                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                Some(block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_decode_uri_component"),
                    &[v], &[i64t], self.loc,
                )).result(0)?.into())
            }
            "encodeURIComponent" => {
                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                Some(block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_encode_uri_component"),
                    &[v], &[i64t], self.loc,
                )).result(0)?.into())
            }
            "decodeURI" => {
                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                Some(block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_decode_uri"),
                    &[v], &[i64t], self.loc,
                )).result(0)?.into())
            }
            "encodeURI" => {
                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                Some(block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_encode_uri"),
                    &[v], &[i64t], self.loc,
                )).result(0)?.into())
            }
            "parseInt" => {
                let s = arg_vals.first().copied().unwrap_or(undef_i64);
                let r = arg_vals.get(1).copied().unwrap_or(undef_i64);
                Some(block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_parse_int"),
                    &[s, r], &[i64t], self.loc,
                )).result(0)?.into())
            }
            "parseFloat" => {
                let s = arg_vals.first().copied().unwrap_or(undef_i64);
                Some(block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_parse_float"),
                    &[s], &[i64t], self.loc,
                )).result(0)?.into())
            }
            "Number" => {
                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                Some(block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_coerce_number"),
                    &[v], &[i64t], self.loc,
                )).result(0)?.into())
            }
            "String" => {
                let v = arg_vals.first().copied().unwrap_or(undef_i64);
                Some(block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_coerce_string"),
                    &[v], &[i64t], self.loc,
                )).result(0)?.into())
            }
            _ => None,
        };
        Ok(result)
    }

}

// ── Free-variable analysis ─────────────────────────────────────────────────────
// Walk arrow-function body to find outer-scope identifiers that must be captured.

type NameSet = std::collections::HashSet<String>;

pub(super) fn collect_free_vars_stmts(
    stmts: &[oxc_ast::ast::Statement<'_>],
    params: &NameSet,
    outer_keys: &NameSet,
    out: &mut Vec<String>,
) {
    let mut local: NameSet = params.clone();
    for stmt in stmts { collect_locals_stmt(stmt, &mut local); }
    for stmt in stmts { collect_free_vars_stmt(stmt, params, &local, outer_keys, out); }
}

fn collect_locals_stmt(stmt: &oxc_ast::ast::Statement<'_>, locals: &mut NameSet) {
    use oxc_ast::ast::Statement;
    match stmt {
        Statement::VariableDeclaration(decl) => {
            for d in &decl.declarations {
                collect_locals_binding(&d.id, locals);
            }
        }
        Statement::FunctionDeclaration(f) => {
            if let Some(id) = &f.id { locals.insert(id.name.to_string()); }
        }
        _ => {}
    }
}

fn collect_locals_binding(pat: &oxc_ast::ast::BindingPattern<'_>, locals: &mut NameSet) {
    use oxc_ast::ast::BindingPattern;
    match pat {
        BindingPattern::BindingIdentifier(id) => { locals.insert(id.name.to_string()); }
        BindingPattern::ObjectPattern(op) => {
            for prop in &op.properties {
                collect_locals_binding(&prop.value, locals);
            }
        }
        BindingPattern::ArrayPattern(ap) => {
            for elem in ap.elements.iter().flatten() {
                collect_locals_binding(elem, locals);
            }
        }
        _ => {}
    }
}

/// Pre-insert a binding pattern's identifier names into scope with a placeholder value.
/// Used to make locally-declared variables visible to hoisted inner function declarations.
fn predeclare_binding<'c, 'b>(
    pat: &oxc_ast::ast::BindingPattern<'_>,
    placeholder: melior::ir::Value<'c, 'b>,
    scope: &mut HashMap<String, melior::ir::Value<'c, 'b>>,
) {
    use oxc_ast::ast::BindingPattern;
    match pat {
        BindingPattern::BindingIdentifier(id) => {
            scope.entry(id.name.to_string()).or_insert(placeholder);
        }
        BindingPattern::ObjectPattern(op) => {
            for prop in &op.properties {
                predeclare_binding(&prop.value, placeholder, scope);
            }
        }
        BindingPattern::ArrayPattern(ap) => {
            for elem in ap.elements.iter().flatten() {
                predeclare_binding(elem, placeholder, scope);
            }
        }
        _ => {}
    }
}

fn collect_free_vars_stmt(
    stmt: &oxc_ast::ast::Statement<'_>,
    params: &NameSet,
    locals: &NameSet,
    outer_keys: &NameSet,
    out: &mut Vec<String>,
) {
    use oxc_ast::ast::Statement;
    match stmt {
        Statement::ExpressionStatement(es) => collect_fv_expr(&es.expression, params, locals, outer_keys, out),
        Statement::ReturnStatement(rs) => {
            if let Some(arg) = &rs.argument {
                collect_fv_expr(arg, params, locals, outer_keys, out);
            }
        }
        Statement::VariableDeclaration(decl) => {
            for d in &decl.declarations {
                if let Some(init) = &d.init {
                    collect_fv_expr(init, params, locals, outer_keys, out);
                }
            }
        }
        Statement::IfStatement(if_stmt) => {
            collect_fv_expr(&if_stmt.test, params, locals, outer_keys, out);
            collect_free_vars_stmt(&if_stmt.consequent, params, locals, outer_keys, out);
            if let Some(alt) = &if_stmt.alternate {
                collect_free_vars_stmt(alt, params, locals, outer_keys, out);
            }
        }
        Statement::BlockStatement(block) => {
            for s in &block.body { collect_free_vars_stmt(s, params, locals, outer_keys, out); }
        }
        Statement::ForOfStatement(for_of) => {
            collect_fv_expr(&for_of.right, params, locals, outer_keys, out);
            collect_free_vars_stmt(&for_of.body, params, locals, outer_keys, out);
        }
        Statement::ForInStatement(for_in) => {
            collect_fv_expr(&for_in.right, params, locals, outer_keys, out);
            collect_free_vars_stmt(&for_in.body, params, locals, outer_keys, out);
        }
        Statement::WhileStatement(w) => {
            collect_fv_expr(&w.test, params, locals, outer_keys, out);
            collect_free_vars_stmt(&w.body, params, locals, outer_keys, out);
        }
        Statement::TryStatement(try_stmt) => {
            for s in &try_stmt.block.body {
                collect_free_vars_stmt(s, params, locals, outer_keys, out);
            }
            if let Some(handler) = &try_stmt.handler {
                for s in &handler.body.body {
                    collect_free_vars_stmt(s, params, locals, outer_keys, out);
                }
            }
            if let Some(fin) = &try_stmt.finalizer {
                for s in &fin.body {
                    collect_free_vars_stmt(s, params, locals, outer_keys, out);
                }
            }
        }
        Statement::FunctionDeclaration(f) => {
            // Inner function declarations transitively capture outer vars.
            // Scan the inner body with the inner function's params + locals excluded.
            if let Some(body) = &f.body {
                let mut inner_locals = locals.clone();
                for param in &f.params.items {
                    if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &param.pattern {
                        inner_locals.insert(id.name.to_string());
                    }
                }
                if let Some(id) = &f.id { inner_locals.insert(id.name.to_string()); }
                for stmt in &body.statements { collect_locals_stmt(stmt, &mut inner_locals); }
                for stmt in &body.statements {
                    collect_free_vars_stmt(stmt, params, &inner_locals, outer_keys, out);
                }
            }
        }
        _ => {}
    }
}

fn collect_fv_expr(
    expr: &oxc_ast::ast::Expression<'_>,
    params: &NameSet,
    locals: &NameSet,
    outer_keys: &NameSet,
    out: &mut Vec<String>,
) {
    use oxc_ast::ast::Expression;
    match expr {
        Expression::Identifier(id) => {
            let name = id.name.as_str();
            if !params.contains(name) && !locals.contains(name) && outer_keys.contains(name) {
                if !out.contains(&name.to_string()) {
                    out.push(name.to_string());
                }
            }
        }
        Expression::ParenthesizedExpression(pe) => {
            collect_fv_expr(&pe.expression, params, locals, outer_keys, out);
        }
        Expression::ThisExpression(_) => {
            // Arrow functions capture `this` from the enclosing lexical scope.
            if outer_keys.contains("this") && !out.contains(&"this".to_string()) {
                out.push("this".to_string());
            }
        }
        Expression::BinaryExpression(bin) => {
            collect_fv_expr(&bin.left, params, locals, outer_keys, out);
            collect_fv_expr(&bin.right, params, locals, outer_keys, out);
        }
        Expression::LogicalExpression(log) => {
            collect_fv_expr(&log.left, params, locals, outer_keys, out);
            collect_fv_expr(&log.right, params, locals, outer_keys, out);
        }
        Expression::UnaryExpression(un) => {
            collect_fv_expr(&un.argument, params, locals, outer_keys, out);
        }
        Expression::AssignmentExpression(assign) => {
            collect_fv_expr(&assign.right, params, locals, outer_keys, out);
            // Also scan the LHS target for `this` (e.g. `this.#field ??= rhs`).
            collect_fv_assignment_target(&assign.left, params, locals, outer_keys, out);
        }
        Expression::CallExpression(call) => {
            collect_fv_expr(&call.callee, params, locals, outer_keys, out);
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() {
                    collect_fv_expr(e, params, locals, outer_keys, out);
                }
            }
        }
        Expression::StaticMemberExpression(m) => {
            collect_fv_expr(&m.object, params, locals, outer_keys, out);
        }
        Expression::ComputedMemberExpression(m) => {
            collect_fv_expr(&m.object, params, locals, outer_keys, out);
            collect_fv_expr(&m.expression, params, locals, outer_keys, out);
        }
        Expression::ConditionalExpression(cond) => {
            collect_fv_expr(&cond.test, params, locals, outer_keys, out);
            collect_fv_expr(&cond.consequent, params, locals, outer_keys, out);
            collect_fv_expr(&cond.alternate, params, locals, outer_keys, out);
        }
        Expression::TemplateLiteral(tmpl) => {
            for e in &tmpl.expressions { collect_fv_expr(e, params, locals, outer_keys, out); }
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                if let Some(e) = elem.as_expression() {
                    collect_fv_expr(e, params, locals, outer_keys, out);
                }
            }
        }
        Expression::ObjectExpression(obj) => {
            use oxc_ast::ast::ObjectPropertyKind;
            for prop in &obj.properties {
                if let ObjectPropertyKind::ObjectProperty(p) = prop {
                    collect_fv_expr(&p.value, params, locals, outer_keys, out);
                }
            }
        }
        // Nested arrow functions: scan their bodies transitively.
        // Variables only used inside the inner arrow still need to be captured by the outer.
        Expression::ArrowFunctionExpression(arrow) => {
            let mut inner_locals = locals.clone();
            for p in &arrow.params.items {
                if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &p.pattern {
                    inner_locals.insert(id.name.to_string());
                }
            }
            if let Some(rest) = &arrow.params.rest {
                if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &rest.rest.argument {
                    inner_locals.insert(id.name.to_string());
                }
            }
            for stmt in &arrow.body.statements {
                collect_locals_stmt(stmt, &mut inner_locals);
            }
            for stmt in &arrow.body.statements {
                collect_free_vars_stmt(stmt, params, &inner_locals, outer_keys, out);
            }
        }
        // TypeScript wrappers: look through to the inner expression
        Expression::TSAsExpression(ts_as) => {
            collect_fv_expr(&ts_as.expression, params, locals, outer_keys, out);
        }
        Expression::TSSatisfiesExpression(ts_sat) => {
            collect_fv_expr(&ts_sat.expression, params, locals, outer_keys, out);
        }
        Expression::TSTypeAssertion(ts_assert) => {
            collect_fv_expr(&ts_assert.expression, params, locals, outer_keys, out);
        }
        Expression::TSNonNullExpression(ts_nn) => {
            collect_fv_expr(&ts_nn.expression, params, locals, outer_keys, out);
        }
        Expression::AwaitExpression(aw) => {
            collect_fv_expr(&aw.argument, params, locals, outer_keys, out);
        }
        Expression::SequenceExpression(seq) => {
            for e in &seq.expressions { collect_fv_expr(e, params, locals, outer_keys, out); }
        }
        Expression::NewExpression(new_expr) => {
            collect_fv_expr(&new_expr.callee, params, locals, outer_keys, out);
            for arg in &new_expr.arguments {
                if let Some(e) = arg.as_expression() {
                    collect_fv_expr(e, params, locals, outer_keys, out);
                }
            }
        }
        Expression::PrivateFieldExpression(m) => {
            collect_fv_expr(&m.object, params, locals, outer_keys, out);
        }
        _ => {}
    }
}

fn collect_fv_assignment_target(
    target: &oxc_ast::ast::AssignmentTarget<'_>,
    params: &NameSet,
    locals: &NameSet,
    outer_keys: &NameSet,
    out: &mut Vec<String>,
) {
    use oxc_ast::ast::AssignmentTarget;
    match target {
        AssignmentTarget::StaticMemberExpression(m) => {
            collect_fv_expr(&m.object, params, locals, outer_keys, out);
        }
        AssignmentTarget::ComputedMemberExpression(m) => {
            collect_fv_expr(&m.object, params, locals, outer_keys, out);
            collect_fv_expr(&m.expression, params, locals, outer_keys, out);
        }
        AssignmentTarget::PrivateFieldExpression(m) => {
            collect_fv_expr(&m.object, params, locals, outer_keys, out);
        }
        _ => {}
    }
}
