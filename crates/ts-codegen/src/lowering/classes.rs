use super::*;

impl<'c, 'm> Lowerer<'c, 'm> {
    // ── Class declarations ────────────────────────────────────────────────

    pub(super) fn lower_class_declaration(&mut self, class: &Class<'_>) -> Result<()> {
        let Some(id) = &class.id else { return Ok(()) };
        let class_name = id.name.to_string();

        self.current_class = Some(class_name.clone());

        self.lower_class_constructor(&class_name, class)?;

        for elem in &class.body.body {
            let ClassElement::MethodDefinition(method) = elem else { continue };
            match (method.kind, method.r#static) {
                (MethodDefinitionKind::Constructor, _) => {}
                (MethodDefinitionKind::Get, false) => {
                    self.lower_class_getter(&class_name, method)?;
                }
                (MethodDefinitionKind::Set, false) => {
                    self.lower_class_setter(&class_name, method)?;
                }
                (MethodDefinitionKind::Method, true) => {
                    self.lower_class_static_method(&class_name, method)?;
                }
                (MethodDefinitionKind::Method, false) => {
                    self.lower_class_method(&class_name, method)?;
                }
                _ => {}
            }
        }

        self.current_class = None;
        self.var_class_types.remove("this");
        Ok(())
    }

    // ── Constructor ───────────────────────────────────────────────────────

    fn lower_class_constructor(&mut self, class_name: &str, class: &Class<'_>) -> Result<()> {
        let constructor = class.body.body.iter().find_map(|elem| {
            if let ClassElement::MethodDefinition(m) = elem {
                if m.kind == MethodDefinitionKind::Constructor { Some(m) } else { None }
            } else {
                None
            }
        });

        let i64_type = self.i64_type();
        let func_name = format!("__class_{}_constructor", class_name);

        // Parent class info
        let parent_name: Option<String> = class.super_class.as_ref().and_then(|e| {
            if let Expression::Identifier(id) = e { Some(id.name.to_string()) } else { None }
        });
        let parent_ctor_name: Option<String> =
            parent_name.as_deref().map(|n| format!("__class_{}_constructor", n));

        let n_params = constructor.map_or(0, |c| c.value.params.items.len());
        let param_specs: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            (0..n_params).map(|_| (i64_type, self.loc)).collect();
        let func_type = FunctionType::new(self.ctx, &vec![i64_type; n_params], &[i64_type]);

        let region = Region::new();
        let entry  = region.append_block(Block::new(&param_specs));
        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();

        // Bind constructor parameters.
        if let Some(ctor) = constructor {
            for (i, param) in ctor.value.params.items.iter().enumerate() {
                if let BindingPattern::BindingIdentifier(id) = &param.pattern {
                    scope.insert(id.name.to_string(), entry.argument(i)?.into());
                }
            }
        }

        let mut current = entry;

        // Locate explicit super() call index in the constructor body.
        let super_call_index: Option<usize> = if parent_ctor_name.is_some() {
            constructor.and_then(|ctor| {
                ctor.value.body.as_ref().and_then(|body| {
                    body.statements.iter().position(|stmt| {
                        if let Statement::ExpressionStatement(es) = stmt {
                            if let Expression::CallExpression(call) = &es.expression {
                                return matches!(&call.callee, Expression::Super(_));
                            }
                        }
                        false
                    })
                })
            })
        } else {
            None
        };

        // Create `this`
        // - No parent:              allocate a fresh object.
        // - Parent + explicit super(): call parent ctor with those args.
        // - Parent + no super():    call parent ctor with no args.
        let this_val: Value<'_, '_> = if let Some(ref parent_ctor) = parent_ctor_name {
            // Extract super() argument expressions (references into the AST).
            let mut call_args: Vec<Value<'_, '_>> = Vec::new();

            if let Some(idx) = super_call_index {
                let ctor_body = constructor.unwrap().value.body.as_ref().unwrap();
                if let Statement::ExpressionStatement(es) = &ctor_body.statements[idx] {
                    if let Expression::CallExpression(call) = &es.expression {
                        for arg in &call.arguments {
                            if let Some(expr) = arg.as_expression() {
                                let (v_opt, nb) =
                                    self.lower_expression(expr, current, &region, &mut scope)?;
                                current = nb;
                                if let Some(v) = v_opt {
                                    call_args.push(self.ensure_i64(v, current)?);
                                }
                            }
                        }
                    }
                }
            }

            current.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, parent_ctor),
                &call_args,
                &[i64_type],
                self.loc,
            )).result(0)?.into()
        } else {
            current.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_obj_new"),
                &[],
                &[i64_type],
                self.loc,
            )).result(0)?.into()
        };
        scope.insert("this".to_string(), this_val);
        self.var_class_types.insert("this".to_string(), class_name.to_string());

        // Store __class__ = class_name for instanceof checks.
        // For inherited objects the child overwrites the parent's tag, which is
        // correct: a Dog's __class__ should be "Dog", not "Animal".
        {
            let class_key_ptr = self.get_string_ptr("__class__", current)?;
            let class_name_str = self.lower_string_literal(class_name, current)?;
            current.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
                &[this_val, class_key_ptr, class_name_str],
                &[],
                self.loc,
            ));
            // ts_obj_set retains class_name_str; we release our constructor-side ref.
            current.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[class_name_str],
                &[],
                self.loc,
            ));
        }

        // Apply own field initialisers after `this` is set.
        for elem in &class.body.body {
            if let ClassElement::PropertyDefinition(prop) = elem {
                if prop.r#static { continue; }
                let Some(init_expr) = &prop.value else { continue };
                let key_str = match &prop.key {
                    oxc_ast::ast::PropertyKey::StaticIdentifier(id) => id.name.to_string(),
                    _ => continue,
                };
                let (val_opt, nb) =
                    self.lower_expression(init_expr, current, &region, &mut scope)?;
                current = nb;
                if let Some(val) = val_opt {
                    let val_i64 = self.ensure_i64(val, current)?;
                    let key_ptr = self.get_string_ptr(&key_str, current)?;
                    let this_i64 = self.ensure_i64(scope["this"], current)?;
                    current.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
                        &[this_i64, key_ptr, val_i64],
                        &[],
                        self.loc,
                    ));
                }
            }
        }

        // Lower constructor body (skip the super() call we already processed).
        self.fn_return_type = i64_type;
        if let Some(ctor) = constructor {
            if let Some(body) = &ctor.value.body {
                for (i, stmt) in body.statements.iter().enumerate() {
                    if Some(i) == super_call_index { continue; }
                    let (_, next) =
                        self.lower_statement(stmt, current, &region, &mut scope, &[])?;
                    current = next;
                }
            }
        }
        self.fn_return_type = self.i32_type();

        // Return `this`.
        if current.terminator().is_none() {
            let final_this = scope.get("this").copied().unwrap_or(this_val);
            let final_this_i64 = self.ensure_i64(final_this, current)?;
            current.append_operation(func::r#return(&[final_this_i64], self.loc));
        }

        self.module.body().append_operation(func::func(
            self.ctx,
            StringAttribute::new(self.ctx, &func_name),
            TypeAttribute::new(func_type.into()),
            region,
            &[],
            self.loc,
        ));

        self.funcs.insert(func_name, FuncSig {
            param_types: vec![i64_type; n_params],
            return_type: Some(i64_type),
        });
        Ok(())
    }

    // ── Instance methods ──────────────────────────────────────────────────

    fn lower_class_method(&mut self, class_name: &str, method: &MethodDefinition<'_>) -> Result<()> {
        let Some(name) = method.key.static_name() else { return Ok(()) };
        let func_name = format!("__class_{}_{}", class_name, name);
        let i64_type  = self.i64_type();

        let n_params = method.value.params.items.len();
        let all_params = 1 + n_params;
        let param_specs: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            (0..all_params).map(|_| (i64_type, self.loc)).collect();
        let func_type = FunctionType::new(self.ctx, &vec![i64_type; all_params], &[i64_type]);

        let region = Region::new();
        let entry  = region.append_block(Block::new(&param_specs));

        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();
        scope.insert("this".to_string(), entry.argument(0)?.into());
        self.var_class_types.insert("this".to_string(), class_name.to_string());
        for (i, param) in method.value.params.items.iter().enumerate() {
            if let BindingPattern::BindingIdentifier(id) = &param.pattern {
                scope.insert(id.name.to_string(), entry.argument(1 + i)?.into());
            }
        }

        let zero_i64: Value<'_, '_> = entry
            .append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(i64_type, 0).into(),
                self.loc,
            ))
            .result(0)?
            .into();

        let mut result = zero_i64;
        let mut current = entry;

        self.fn_return_type = i64_type;
        if let Some(body) = &method.value.body {
            for stmt in &body.statements {
                let (val, next) = self.lower_statement(stmt, current, &region, &mut scope, &[])?;
                current = next;
                if let Some(v) = val { result = v; }
            }
        }
        self.fn_return_type = self.i32_type();

        if current.terminator().is_none() {
            let result_i64 = self.ensure_i64(result, current)?;
            current.append_operation(func::r#return(&[result_i64], self.loc));
        }

        self.module.body().append_operation(func::func(
            self.ctx,
            StringAttribute::new(self.ctx, &func_name),
            TypeAttribute::new(func_type.into()),
            region,
            &[],
            self.loc,
        ));

        self.funcs.insert(func_name, FuncSig {
            param_types: vec![i64_type; all_params],
            return_type: Some(i64_type),
        });
        Ok(())
    }

    // ── Getter ────────────────────────────────────────────────────────────

    fn lower_class_getter(&mut self, class_name: &str, method: &MethodDefinition<'_>) -> Result<()> {
        let Some(prop_name) = method.key.static_name() else { return Ok(()) };
        let func_name = format!("__class_{}_get_{}", class_name, prop_name);
        let i64_type  = self.i64_type();

        let param_specs = vec![(i64_type, self.loc)];
        let func_type = FunctionType::new(self.ctx, &[i64_type], &[i64_type]);

        let region = Region::new();
        let entry  = region.append_block(Block::new(&param_specs));

        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();
        scope.insert("this".to_string(), entry.argument(0)?.into());
        self.var_class_types.insert("this".to_string(), class_name.to_string());

        let zero_i64: Value<'_, '_> = entry
            .append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64_type, 0).into(), self.loc,
            )).result(0)?.into();

        let mut result = zero_i64;
        let mut current = entry;

        self.fn_return_type = i64_type;
        if let Some(body) = &method.value.body {
            for stmt in &body.statements {
                let (val, next) = self.lower_statement(stmt, current, &region, &mut scope, &[])?;
                current = next;
                if let Some(v) = val { result = v; }
            }
        }
        self.fn_return_type = self.i32_type();

        if current.terminator().is_none() {
            let result_i64 = self.ensure_i64(result, current)?;
            current.append_operation(func::r#return(&[result_i64], self.loc));
        }

        self.module.body().append_operation(func::func(
            self.ctx,
            StringAttribute::new(self.ctx, &func_name),
            TypeAttribute::new(func_type.into()),
            region,
            &[],
            self.loc,
        ));

        self.funcs.insert(func_name.clone(), FuncSig {
            param_types: vec![i64_type],
            return_type: Some(i64_type),
        });
        Ok(())
    }

    // ── Setter ────────────────────────────────────────────────────────────

    fn lower_class_setter(&mut self, class_name: &str, method: &MethodDefinition<'_>) -> Result<()> {
        let Some(prop_name) = method.key.static_name() else { return Ok(()) };
        let func_name = format!("__class_{}_set_{}", class_name, prop_name);
        let i64_type  = self.i64_type();

        // (this: i64, value: i64) → i64  (returns 0; MLIR requires a result)
        let param_specs = vec![(i64_type, self.loc), (i64_type, self.loc)];
        let func_type = FunctionType::new(self.ctx, &[i64_type, i64_type], &[i64_type]);

        let region = Region::new();
        let entry  = region.append_block(Block::new(&param_specs));

        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();
        scope.insert("this".to_string(), entry.argument(0)?.into());
        self.var_class_types.insert("this".to_string(), class_name.to_string());
        if let Some(param) = method.value.params.items.first() {
            if let BindingPattern::BindingIdentifier(id) = &param.pattern {
                scope.insert(id.name.to_string(), entry.argument(1)?.into());
            }
        }

        let zero_i64: Value<'_, '_> = entry
            .append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64_type, 0).into(), self.loc,
            )).result(0)?.into();

        let mut current = entry;

        self.fn_return_type = i64_type;
        if let Some(body) = &method.value.body {
            for stmt in &body.statements {
                let (_, next) = self.lower_statement(stmt, current, &region, &mut scope, &[])?;
                current = next;
            }
        }
        self.fn_return_type = self.i32_type();

        if current.terminator().is_none() {
            current.append_operation(func::r#return(&[zero_i64], self.loc));
        }

        self.module.body().append_operation(func::func(
            self.ctx,
            StringAttribute::new(self.ctx, &func_name),
            TypeAttribute::new(func_type.into()),
            region,
            &[],
            self.loc,
        ));

        self.funcs.insert(func_name, FuncSig {
            param_types: vec![i64_type, i64_type],
            return_type: Some(i64_type),
        });
        Ok(())
    }

    // ── Static method ─────────────────────────────────────────────────────

    fn lower_class_static_method(
        &mut self,
        class_name: &str,
        method: &MethodDefinition<'_>,
    ) -> Result<()> {
        let Some(name) = method.key.static_name() else { return Ok(()) };
        let func_name = format!("__class_{}_static_{}", class_name, name);
        let i64_type  = self.i64_type();

        let n_params = method.value.params.items.len();
        let param_specs: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            (0..n_params).map(|_| (i64_type, self.loc)).collect();
        let func_type = FunctionType::new(self.ctx, &vec![i64_type; n_params], &[i64_type]);

        let region = Region::new();
        let entry  = region.append_block(Block::new(&param_specs));

        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();
        for (i, param) in method.value.params.items.iter().enumerate() {
            if let BindingPattern::BindingIdentifier(id) = &param.pattern {
                scope.insert(id.name.to_string(), entry.argument(i)?.into());
            }
        }

        let zero_i64: Value<'_, '_> = entry
            .append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64_type, 0).into(), self.loc,
            )).result(0)?.into();

        let mut result = zero_i64;
        let mut current = entry;

        self.fn_return_type = i64_type;
        if let Some(body) = &method.value.body {
            for stmt in &body.statements {
                let (val, next) = self.lower_statement(stmt, current, &region, &mut scope, &[])?;
                current = next;
                if let Some(v) = val { result = v; }
            }
        }
        self.fn_return_type = self.i32_type();

        if current.terminator().is_none() {
            let result_i64 = self.ensure_i64(result, current)?;
            current.append_operation(func::r#return(&[result_i64], self.loc));
        }

        self.module.body().append_operation(func::func(
            self.ctx,
            StringAttribute::new(self.ctx, &func_name),
            TypeAttribute::new(func_type.into()),
            region,
            &[],
            self.loc,
        ));

        self.funcs.insert(func_name, FuncSig {
            param_types: vec![i64_type; n_params],
            return_type: Some(i64_type),
        });
        Ok(())
    }
}
