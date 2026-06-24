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
            Expression::TSNonNullExpression(ts_nn) => {
                self.lower_expression(&ts_nn.expression, block, region, scope)
            }
            Expression::StaticMemberExpression(member) => {
                // global.Promise → ts_get_promise_constructor(); global.Buffer → ts_get_buffer_constructor()
                if let Expression::Identifier(obj_id) = &member.object {
                    if obj_id.name == "global" || obj_id.name == "globalThis" {
                        let rt_name = match member.property.name.as_str() {
                            "Promise" => Some("ts_get_promise_constructor"),
                            "Buffer"  => Some("ts_get_buffer_constructor"),
                            _ => None,
                        };
                        if let Some(rt) = rt_name {
                            let val: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, rt),
                                &[], &[self.i64_type()], self.loc,
                            )).result(0)?.into();
                            return Ok((Some(val), block));
                        }
                        // Unknown global property — return UNDEFINED
                        let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                            self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                        )).result(0)?.into();
                        return Ok((Some(undef), block));
                    }
                }
                // process.argv → ts_process_argv(); process.env → ts_process_env()
                if let Expression::Identifier(obj_id) = &member.object {
                    if obj_id.name == "process" {
                        let rt_name = match member.property.name.as_str() {
                            "argv"     => Some("ts_process_argv"),
                            "env"      => Some("ts_process_env"),
                            "pid"      => Some("ts_process_pid"),
                            "platform" => Some("ts_process_platform"),
                            "version"  => Some("ts_process_version"),
                            "versions" => Some("ts_process_versions"),
                            // process.stderr / process.stdout — return UNDEFINED;
                            // write() on these objects will be handled by the callee dispatch.
                            "stderr" | "stdout" => None,
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

                // Math constants: Math.PI, Math.E, Math.LN2, …
                if let Expression::Identifier(obj_id) = &member.object {
                    if obj_id.name == "Math" {
                        let prop = member.property.name.as_str();
                        let f = match prop {
                            "PI"      => Some(std::f64::consts::PI),
                            "E"       => Some(std::f64::consts::E),
                            "LN2"     => Some(std::f64::consts::LN_2),
                            "LN10"    => Some(std::f64::consts::LN_10),
                            "LOG2E"   => Some(std::f64::consts::LOG2_E),
                            "LOG10E"  => Some(std::f64::consts::LOG10_E),
                            "SQRT2"   => Some(std::f64::consts::SQRT_2),
                            "SQRT1_2" => Some(std::f64::consts::FRAC_1_SQRT_2),
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

                // Number constants: Number.MAX_VALUE, Number.MIN_VALUE, etc.
                if let Expression::Identifier(obj_id) = &member.object {
                    if obj_id.name == "Number" {
                        let prop = member.property.name.as_str();
                        let fval = match prop {
                            "MAX_VALUE"         => Some(f64::MAX),
                            "MIN_VALUE"         => Some(f64::MIN_POSITIVE),
                            "EPSILON"           => Some(f64::EPSILON),
                            "MAX_SAFE_INTEGER"  => Some(9007199254740991.0_f64),
                            "MIN_SAFE_INTEGER"  => Some(-9007199254740991.0_f64),
                            "POSITIVE_INFINITY" => Some(f64::INFINITY),
                            "NEGATIVE_INFINITY" => Some(f64::NEG_INFINITY),
                            "NaN"               => Some(f64::NAN),
                            _ => None,
                        };
                        if let Some(v) = fval {
                            let lit: Value<'c, 'b> = block.append_operation(arith::constant(
                                self.ctx,
                                IntegerAttribute::new(self.i64_type(), v.to_bits() as i64).into(),
                                self.loc,
                            )).result(0)?.into();
                            return Ok((Some(lit), block));
                        }
                    }
                }

                // Symbol well-known values: Symbol.iterator → ts_symbol_iterator()
                if let Expression::Identifier(obj_id) = &member.object {
                    if obj_id.name == "Symbol" && member.property.name.as_str() == "iterator" {
                        let sym_val: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_symbol_iterator"),
                            &[],
                            &[self.i64_type()],
                            self.loc,
                        )).result(0)?.into();
                        return Ok((Some(sym_val), block));
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

                // Static class field read: ClassName.fieldName → ts_get_module_global("__static_ClassName_fieldName")
                if let Expression::Identifier(obj_id) = &member.object {
                    let class_name = obj_id.name.as_str();
                    let field_name = member.property.name.as_str();
                    let is_static_field = self.classes.get(class_name)
                        .map(|sig| sig.static_fields.contains(field_name))
                        .unwrap_or(false);
                    if is_static_field {
                        let global_key = format!("__static_{}_{}", class_name, field_name);
                        let key_ptr = self.get_string_ptr(&global_key, block)?;
                        let val: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_get_module_global"),
                            &[key_ptr], &[self.i64_type()], self.loc,
                        )).result(0)?.into();
                        return Ok((Some(val), block));
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
                let obj = match obj_opt {
                    Some(v) => v,
                    None => { let u: Value<'c,'b> = block.append_operation(arith::constant(self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc)).result(0)?.into(); u }
                };

                // .size  →  ts_container_size(obj)  (Map, Set, URLSearchParams, …)
                if member.property.name == "size" {
                    let obj_i64 = self.ensure_i64(obj, block)?;
                    let sz: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_container_size"),
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

                // Response property dispatch: .status, .ok, .headers
                match member.property.name.as_str() {
                    "status" => {
                        let val: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_response_status"),
                            &[obj_i64], &[self.i64_type()], self.loc,
                        )).result(0)?.into();
                        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                        return Ok((Some(val), block));
                    }
                    "ok" => {
                        let val: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_response_ok"),
                            &[obj_i64], &[self.i64_type()], self.loc,
                        )).result(0)?.into();
                        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                        return Ok((Some(val), block));
                    }
                    "headers" => {
                        let val: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_response_headers"),
                            &[obj_i64], &[self.i64_type()], self.loc,
                        )).result(0)?.into();
                        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                        return Ok((Some(val), block));
                    }
                    _ => {}
                }

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
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[obj_i64], &[], self.loc,
                ));

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
                        // If this variable is a cell, read the actual value through the cell.
                        if self.is_cell_var(&name) {
                            let val = self.cell_read(v, block)?;
                            return Ok((Some(val), block));
                        }
                        // Skip ts_retain_val for proven scalars — it's a no-op for non-pointers.
                        if self.scalar_vars.contains(&name) {
                            return Ok((Some(v), block));
                        }
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
                        // Also resolve import aliases (e.g. `import { X as Y }` → look up "X").
                        let global_lookup = if self.module_global_names.contains(&name) {
                            Some(name.clone())
                        } else if let Some(original) = self.module_global_aliases.get(&name).cloned() {
                            if self.module_global_names.contains(&original) { Some(original) } else { None }
                        } else {
                            None
                        };
                        if let Some(global_name) = global_lookup {
                            let key_ptr = self.get_string_ptr(&global_name, block)?;
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
                        // Also resolve import aliases (e.g. `import { parse as pathParse }` → "parse").
                        let func_key = if self.funcs.contains_key(&name) {
                            name.clone()
                        } else if let Some(alias_target) = self.module_global_aliases.get(&name) {
                            if self.funcs.contains_key(alias_target.as_str()) { alias_target.clone() } else { name.clone() }
                        } else { name.clone() };
                        if self.funcs.contains_key(&func_key) {
                            let name = func_key;
                            let i64t = self.i64_type();
                            let i32t = self.i32_type();
                            let sig = self.funcs[&name].clone();
                            // Arity for TsFunction excludes the implicit `this` MLIR param.
                            let this_offset = if sig.has_this_param { 1 } else { 0 };
                            let arity = (sig.param_types.len() - this_offset) as i64;
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
                            let constructor_name = if sig.has_this_param { "ts_func_new_this" } else { "ts_func_new" };
                            let fn_val: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, constructor_name),
                                &[fn_ptr, arity_val], &[i64t], self.loc,
                            )).result(0)?.into();
                            return Ok((Some(fn_val), block));
                        }
                        // Global constants available in TypeScript/JavaScript.
                        let global_val: Option<Value<'c, 'b>> = match name.as_str() {
                            "Infinity" => {
                                Some(block.append_operation(arith::constant(
                                    self.ctx,
                                    IntegerAttribute::new(self.i64_type(), f64::INFINITY.to_bits() as i64).into(),
                                    self.loc,
                                )).result(0)?.into())
                            }
                            "NaN" => {
                                // Use a NaN bit pattern that is NOT NaN-boxed (bit 51 = 0),
                                // so it passes through as a plain IEEE-754 NaN double.
                                // 0x7FF0_0000_0000_0001: exp=all-1, mantissa≠0 → NaN; bit51=0 → not nan-boxed.
                                Some(block.append_operation(arith::constant(
                                    self.ctx,
                                    IntegerAttribute::new(self.i64_type(), 0x7FF0_0000_0000_0001u64 as i64).into(),
                                    self.loc,
                                )).result(0)?.into())
                            }
                            "undefined" => {
                                Some(block.append_operation(arith::constant(
                                    self.ctx,
                                    IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(),
                                    self.loc,
                                )).result(0)?.into())
                            }
                            // Namespace identifiers used as values (e.g. Function.prototype.toString.call(Object)).
                            // These are normally only meaningful as static dispatch namespaces; when accessed
                            // as a value, return UNDEFINED (safe fallback — code paths using these as values
                            // are typically utility checks like isPlainObject that work with any falsy/truthy).
                            "Object" | "Function" | "Array" | "Math" | "JSON" | "Promise"
                            | "Number" | "String" | "Boolean" | "Symbol" | "Date" | "RegExp"
                            | "Error" | "Reflect" | "process" | "console" => {
                                Some(block.append_operation(arith::constant(
                                    self.ctx,
                                    IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(),
                                    self.loc,
                                )).result(0)?.into())
                            }
                            _ => None,
                        };
                        if let Some(v) = global_val {
                            return Ok((Some(v), block));
                        }
                        // Fallback: treat as a module global that wasn't registered yet
                        // (can happen with circular imports where signature collection
                        // order doesn't match lowering order). Emit ts_get_module_global
                        // so it's resolved at runtime.
                        let key_ptr = self.get_string_ptr(&name, block)?;
                        let val: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_get_module_global"),
                            &[key_ptr], &[self.i64_type()], self.loc,
                        )).result(0)?.into();
                        return Ok((Some(val), block));
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
                    None => {
                        // `this` in a plain function expression (not a class method) — no bound
                        // receiver. Return UNDEFINED, which is the correct result when calling
                        // strict-mode functions or when the code is purely for side effects.
                        let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                            self.ctx,
                            IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(),
                            self.loc,
                        )).result(0)?.into();
                        Ok((Some(undef), block))
                    }
                }
            }
            Expression::MetaProperty(meta) => {
                // import.meta — return an object with url/dirname/filename/env.
                // new.target — return undefined (not supported in this compiler).
                if meta.meta.name.as_str() == "import" && meta.property.name.as_str() == "meta" {
                    let val: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_import_meta_new"),
                        &[], &[self.i64_type()], self.loc,
                    )).result(0)?.into();
                    Ok((Some(val), block))
                } else {
                    // new.target or unknown — return undefined
                    let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                        self.ctx,
                        IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(),
                        self.loc,
                    )).result(0)?.into();
                    Ok((Some(undef), block))
                }
            }
            Expression::YieldExpression(y) => {
                // `yield expr` in a generator function:
                // Push the value to the __generator_yields array and return undefined.
                let i32_type = self.i32_type();
                let i64_type = self.i64_type();
                let yields_cell = if let Some(v) = scope.get("__generator_yields").copied() {
                    v
                } else {
                    // yield outside a recognized generator context (e.g., from a JS-only package stub).
                    // Evaluate the argument for side-effects and return undefined.
                    if let Some(arg) = &y.argument {
                        let (v_opt, nb) = self.lower_expression(arg, block, region, scope)?;
                        block = nb;
                        if let Some(v) = v_opt {
                            let v_i64 = self.ensure_i64(v, block)?;
                            block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                &[v_i64], &[], self.loc,
                            ));
                        }
                    }
                    let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                        self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                    )).result(0)?.into();
                    return Ok((Some(undef), block));
                };
                let yields_i64 = self.ensure_i64(yields_cell, block)?;

                let yield_val: Value<'c, 'b> = if let Some(arg) = &y.argument {
                    let (v_opt, nb) = self.lower_expression(arg, block, region, scope)?;
                    block = nb;
                    let v = v_opt.ok_or_else(|| anyhow::anyhow!("yield arg produced no value"))?;
                    self.ensure_i64(v, block)?
                } else {
                    block.append_operation(arith::constant(
                        self.ctx,
                        IntegerAttribute::new(i64_type, 0x7FF8_0000_0000_0000u64 as i64).into(),
                        self.loc,
                    )).result(0)?.into()
                };

                // If delegate (yield*), push all elements from the iterable
                if y.delegate {
                    let len_val: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_iterable_len"),
                        &[yield_val], &[i64_type], self.loc,
                    )).result(0)?.into();
                    block.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push_all"),
                        &[yields_i64, yield_val], &[], self.loc,
                    ));
                    block.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[yield_val], &[], self.loc,
                    ));
                    let _ = len_val;
                } else {
                    block.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push"),
                        &[yields_i64, yield_val], &[], self.loc,
                    ));
                    block.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[yield_val], &[], self.loc,
                    ));
                }

                // yield expression evaluates to undefined (we don't support .next(value))
                let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx,
                    IntegerAttribute::new(i64_type, 0x7FF8_0000_0000_0000u64 as i64).into(),
                    self.loc,
                )).result(0)?.into();
                Ok((Some(undef), block))
            }
            Expression::AwaitExpression(aw) => {
                let (val_opt, nb) = self.lower_expression(&aw.argument, block, region, scope)?;
                block = nb;
                let val = match val_opt {
                    Some(v) => v,
                    None => {
                        // Awaited expression produced no value (e.g., from a skipped JS package).
                        block.append_operation(arith::constant(
                            self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                        )).result(0)?.into()
                    }
                };
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
            Expression::TaggedTemplateExpression(tagged) => {
                self.lower_tagged_template(tagged, block, region, scope)
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
            // Sequence expression (comma operator): evaluate all, return last value.
            Expression::SequenceExpression(seq) => {
                let mut last: Option<Value<'c, 'b>> = None;
                for (i, expr) in seq.expressions.iter().enumerate() {
                    let (val_opt, nb) = self.lower_expression(expr, block, region, scope)?;
                    block = nb;
                    if i + 1 < seq.expressions.len() {
                        // Release intermediate values (side-effects only).
                        if let Some(v) = val_opt {
                            let v_i64 = self.ensure_i64(v, block)?;
                            block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                &[v_i64], &[], self.loc,
                            ));
                        }
                    } else {
                        last = val_opt;
                    }
                }
                Ok((last, block))
            }
            Expression::ImportExpression(import_expr) => {
                // Dynamic import('specifier') — resolve at compile time (AOT), return a
                // resolved Promise wrapping the CJS namespace object.
                // The module was already loaded by collect_local_imports (which also
                // processes import() specifiers). At runtime we wrap the namespace in a
                // resolved Promise so `await import('x')` works.
                let spec_str = match &import_expr.source {
                    Expression::StringLiteral(s) => s.value.to_string(),
                    other => {
                        let (v, nb) = self.lower_expression(other, block, region, scope)?;
                        block = nb;
                        // Non-literal dynamic import: can't resolve statically; return UNDEFINED promise
                        let undef = v.unwrap_or_else(|| block.append_operation(arith::constant(
                            self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                        )).result(0).unwrap().into());
                        let undef_i64 = self.ensure_i64(undef, block)?;
                        let promise: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_promise_resolve"),
                            &[undef_i64], &[self.i64_type()], self.loc,
                        )).result(0)?.into();
                        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[undef_i64], &[], self.loc));
                        return Ok((Some(promise), block));
                    }
                };
                // Get the namespace object for the already-compiled module
                let spec_val = self.lower_string_literal(&spec_str, block)?;
                let ns: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_cjs_require_ns"),
                    &[spec_val], &[self.i64_type()], self.loc,
                )).result(0)?.into();
                // Wrap in a resolved Promise
                let promise: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_promise_resolve"),
                    &[ns], &[self.i64_type()], self.loc,
                )).result(0)?.into();
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[ns], &[], self.loc));
                Ok((Some(promise), block))
            }
            _ => {
                tracing::debug!("skipping unimplemented expression kind");
                Ok((None, block))
            }
        }
    }
}

impl<'c, 'm> Lowerer<'c, 'm> {
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

    // ── Tagged template literals ───────────────────────────────────────────
    // tag`strings ${expr}` → tag([strings], expr)
    // The tag function receives: an array of string parts, then each interpolated value.
    // The strings array has a `.raw` property equal to the cooked strings (simplified).

    pub(super) fn lower_tagged_template<'b>(
        &mut self,
        tagged: &oxc_ast::ast::TaggedTemplateExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let i64t = self.i64_type();
        let undefined_i64: Value<'c, 'b> = block.append_operation(arith::constant(
            self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
        )).result(0)?.into();
        let zero_i32: Value<'c, 'b> = block.append_operation(arith::constant(
            self.ctx, IntegerAttribute::new(self.i32_type(), 0).into(), self.loc,
        )).result(0)?.into();

        // Build the strings array from quasi cooked values.
        let strings_arr: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
            &[zero_i32], &[i64t], self.loc,
        )).result(0)?.into();
        for quasi in &tagged.quasi.quasis {
            let cooked = quasi.value.cooked.as_deref().unwrap_or("");
            let s = self.lower_string_literal(cooked, block)?;
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push"),
                &[strings_arr, s], &[], self.loc,
            ));
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[s], &[], self.loc,
            ));
        }

        // Evaluate each interpolated expression.
        let mut expr_vals: Vec<Value<'c, 'b>> = Vec::new();
        for expr in &tagged.quasi.expressions {
            let (val_opt, nb) = self.lower_expression(expr, block, region, scope)?;
            block = nb;
            let val = val_opt.unwrap_or(undefined_i64);
            expr_vals.push(self.ensure_i64(val, block)?);
        }

        // Evaluate the tag expression.
        let (tag_opt, nb) = self.lower_expression(&tagged.tag, block, region, scope)?;
        block = nb;
        let tag_fn = tag_opt.unwrap_or(undefined_i64);
        let tag_i64 = self.ensure_i64(tag_fn, block)?;

        // Build args: [strings_arr, expr0, expr1, ...]
        let mut all_args: Vec<Value<'c, 'b>> = vec![strings_arr];
        all_args.extend_from_slice(&expr_vals);

        // Call via ts_func_call_n or ts_func_spread_call.
        // Build a TsArray of all args and use dispatch_callback for simplicity.
        let args_arr: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
            &[zero_i32], &[i64t], self.loc,
        )).result(0)?.into();
        for &av in &all_args {
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push"),
                &[args_arr, av], &[], self.loc,
            ));
        }
        let result: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_func_spread_call"),
            &[tag_i64, args_arr], &[i64t], self.loc,
        )).result(0)?.into();

        // Release all temporaries.
        block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[strings_arr], &[], self.loc,
        ));
        for &av in &expr_vals {
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[av], &[], self.loc,
            ));
        }
        block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[tag_i64], &[], self.loc,
        ));
        block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[args_arr], &[], self.loc,
        ));

        Ok((Some(result), block))
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
        let (block_body, expr_body) = if arrow.expression {
            (None, Some(&*arrow.body))
        } else {
            (Some(&*arrow.body), None)
        };
        let (fn_val, nb) = self.lower_arrow_like(&params, rest_name, block_body, expr_body, block, region, scope)?;
        Ok((Some(fn_val), nb))
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

pub(crate) use super::free_vars::{collect_free_vars_stmts, compute_cell_vars_for_body, body_uses_arguments};
pub(crate) use super::analysis::{compute_scalar_vars_for_body, compute_non_escaping_allocs};
