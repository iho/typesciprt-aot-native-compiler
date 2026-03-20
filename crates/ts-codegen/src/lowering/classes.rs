use super::*;

impl<'c, 'm> Lowerer<'c, 'm> {
    // ── Class declarations ────────────────────────────────────────────────

    pub(super) fn lower_class_declaration_with_name(&mut self, class_name: &str, class: &Class<'_>) -> Result<()> {
        if self.lowered_classes.contains(class_name) {
            return Ok(());
        }
        self.lowered_classes.insert(class_name.to_string());

        // If the class is exported under an alias (e.g. `class Hono` → exported as `HonoBase`),
        // register a scoped mapping so that `new Hono(...)` inside the class body resolves correctly.
        // The mapping is removed after the class is fully lowered so it doesn't bleed into other files.
        let scoped_alias: Option<String> = if let Some(id) = &class.id {
            let original_name = id.name.to_string();
            if original_name != class_name {
                self.class_name_aliases.insert(original_name.clone(), class_name.to_string());
                Some(original_name)
            } else {
                None
            }
        } else {
            None
        };

        // Register in self.classes if not already present (needed for method dispatch)
        if !self.classes.contains_key(class_name) {
            let parent = class.super_class.as_ref().and_then(|e| {
                if let Expression::Identifier(id) = e { Some(id.name.to_string()) } else { None }
            });
            let mut methods: HashMap<String, String> = HashMap::new();
            let mut method_arity: HashMap<String, usize> = HashMap::new();
            let mut statics: HashMap<String, String> = HashMap::new();
            let mut getters: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut setters: std::collections::HashSet<String> = std::collections::HashSet::new();
            let builtin_error_names = ["Error", "TypeError", "RangeError", "ReferenceError", "SyntaxError"];
            let ctor_elem = class.body.body.iter().find(|e| {
                matches!(e, ClassElement::MethodDefinition(m) if m.kind == MethodDefinitionKind::Constructor && m.value.body.is_some())
            });
            let constructor_arity = match ctor_elem {
                Some(ClassElement::MethodDefinition(m)) => m.value.params.items.len(),
                _ => {
                    if parent.as_deref().map(|n| builtin_error_names.contains(&n)).unwrap_or(false) { 1 } else { 0 }
                }
            };
            for elem in &class.body.body {
                let ClassElement::MethodDefinition(method) = elem else { continue };
                if method.value.body.is_none() { continue; }
                let name_opt: Option<String> = match &method.key {
                    oxc_ast::ast::PropertyKey::StaticIdentifier(id) => Some(id.name.to_string()),
                    oxc_ast::ast::PropertyKey::PrivateIdentifier(id) => Some(format!("__priv_{}", id.name.as_str())),
                    _ => method.key.static_name().map(|n| n.to_string()),
                };
                let Some(mname) = name_opt else { continue };
                match (method.kind, method.r#static) {
                    (MethodDefinitionKind::Get, false) => { getters.insert(mname); }
                    (MethodDefinitionKind::Set, false) => { setters.insert(mname); }
                    (MethodDefinitionKind::Method, true) => { statics.insert(mname.clone(), format!("__class_{}_{}", class_name, mname)); }
                    (MethodDefinitionKind::Method, false) => {
                        let rest = if method.value.params.rest.is_some() { 1 } else { 0 };
                        let arity = 1 + method.value.params.items.len() + rest;
                        method_arity.insert(mname.clone(), arity);
                        methods.insert(mname.clone(), format!("__class_{}_{}", class_name, mname));
                    }
                    _ => {}
                }
            }
            // Inherit from parent
            let parent_sig = parent.as_ref().and_then(|p| self.classes.get(p)).cloned();
            if let Some(psig) = parent_sig {
                for (k, v) in &psig.methods { methods.entry(k.clone()).or_insert_with(|| v.clone()); }
                for (k, v) in &psig.method_arity { method_arity.entry(k.clone()).or_insert(*v); }
                for (k, v) in &psig.statics { statics.entry(k.clone()).or_insert_with(|| v.clone()); }
                for k in &psig.getters { getters.insert(k.clone()); }
                for k in &psig.setters { setters.insert(k.clone()); }
            }
            let mut static_fields: std::collections::HashSet<String> = std::collections::HashSet::new();
            for elem in &class.body.body {
                if let ClassElement::PropertyDefinition(prop) = elem {
                    if prop.r#static {
                        if let Some(name) = prop.key.static_name() {
                            static_fields.insert(name.to_string());
                        }
                    }
                }
            }
            self.classes.insert(class_name.to_string(), ClassSig {
                constructor_name: format!("__class_{}_constructor", class_name),
                constructor_arity,
                methods,
                method_arity,
                statics,
                getters,
                setters,
                static_fields,
                parent,
            });
        }

        self.current_class = Some(class_name.to_string());

        // Pre-register all class methods in self.funcs BEFORE lowering the constructor.
        // This is needed so that field initializer arrow functions (e.g. `fetch = (req) => this.#dispatch(...)`)
        // can resolve `this.#method()` as a direct function call instead of a dynamic property lookup.
        let i64_type = self.i64_type();
        for elem in &class.body.body {
            let ClassElement::MethodDefinition(method) = elem else { continue };
            if method.value.body.is_none() { continue; }
            let mname_opt: Option<String> = match &method.key {
                oxc_ast::ast::PropertyKey::StaticIdentifier(id) => Some(id.name.to_string()),
                oxc_ast::ast::PropertyKey::PrivateIdentifier(id) => Some(format!("__priv_{}", id.name.as_str())),
                _ => method.key.static_name().map(|n| n.to_string()),
            };
            let Some(mname) = mname_opt else { continue };
            let func_name = format!("__class_{}_{}", class_name, mname);
            let has_rest = method.value.params.rest.is_some();
            let n_params = method.value.params.items.len();
            let all_params = 1 + n_params + if has_rest { 1 } else { 0 }; // +1 for `this`, +1 for rest array
            if !self.funcs.contains_key(&func_name) {
                self.funcs.insert(func_name, FuncSig {
                    param_types: vec![i64_type; all_params],
                    return_type: Some(i64_type),
                    has_rest,
                    has_this_param: false,
                });
            }
        }

        self.lower_class_constructor(class_name, class)?;
        for elem in &class.body.body {
            let ClassElement::MethodDefinition(method) = elem else { continue };
            if method.value.body.is_none() { continue; }
            match (method.kind, method.r#static) {
                (MethodDefinitionKind::Constructor, _) => {}
                (MethodDefinitionKind::Get, false) => { self.lower_class_getter(class_name, method)?; }
                (MethodDefinitionKind::Set, false) => { self.lower_class_setter(class_name, method)?; }
                (MethodDefinitionKind::Method, true) => { self.lower_class_static_method(class_name, method)?; }
                (MethodDefinitionKind::Method, false) => { self.lower_class_method(class_name, method)?; }
                _ => {}
            }
        }
        self.current_class = None;
        self.var_class_types.remove("this");

        // Emit __init_static_ClassName function for static property initializers.
        // Each static field `static count = 0` is stored as a module global `__static_ClassName_count`.
        let static_prop_defs: Vec<(String, &oxc_ast::ast::Expression<'_>)> = class.body.body.iter()
            .filter_map(|elem| {
                if let ClassElement::PropertyDefinition(prop) = elem {
                    if prop.r#static {
                        if let Some(init) = &prop.value {
                            if let Some(name) = prop.key.static_name() {
                                return Some((name.to_string(), init));
                            }
                        }
                    }
                }
                None
            })
            .collect();

        if !static_prop_defs.is_empty() {
            let fn_name = format!("__init_static_{}", class_name);
            let i64_type = self.i64_type();
            let fn_type = FunctionType::new(self.ctx, &[], &[]);
            let region = Region::new();
            let entry = region.append_block(Block::new(&[]));
            let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();
            let mut current = entry;

            // Inject any known module globals so initializers can reference them.
            for global_name in self.module_global_names.clone() {
                if !scope.contains_key(&global_name) {
                    let key_ptr = self.get_string_ptr(&global_name, current)?;
                    let val: Value<'_, '_> = current.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_get_module_global"),
                        &[key_ptr], &[i64_type], self.loc,
                    )).result(0)?.into();
                    scope.insert(global_name, val);
                }
            }

            for (field_name, init_expr) in &static_prop_defs {
                let (val_opt, nb) = self.lower_expression(init_expr, current, &region, &mut scope)?;
                current = nb;
                if let Some(val) = val_opt {
                    let val_i64 = self.ensure_i64(val, current)?;
                    let global_key = format!("__static_{}_{}", class_name, field_name);
                    let key_ptr = self.get_string_ptr(&global_key, current)?;
                    current.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_set_module_global"),
                        &[key_ptr, val_i64], &[], self.loc,
                    ));
                    // ts_set_module_global retains; release our reference.
                    current.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[val_i64], &[], self.loc,
                    ));
                }
            }

            // Release injected module globals.
            for (name, v) in &scope {
                if self.module_global_names.contains(name) {
                    let v_i64 = self.ensure_i64(*v, current)?;
                    current.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[v_i64], &[], self.loc,
                    ));
                }
            }

            current.append_operation(func::r#return(&[], self.loc));

            self.module.body().append_operation(func::func(
                self.ctx,
                StringAttribute::new(self.ctx, &fn_name),
                TypeAttribute::new(fn_type.into()),
                region,
                &[],
                self.loc,
            ));
            self.funcs.insert(fn_name.clone(), FuncSig {
                param_types: vec![],
                return_type: None,
                has_rest: false,
                has_this_param: false,
            });
            self.module_init_fns.push(fn_name);
        }

        // Remove the scoped alias now that the class body is fully lowered.
        if let Some(original_name) = scoped_alias {
            self.class_name_aliases.remove(&original_name);
        }
        Ok(())
    }

    pub(super) fn lower_class_declaration(&mut self, class: &Class<'_>) -> Result<()> {
        let Some(id) = &class.id else { return Ok(()) };
        let class_name = id.name.to_string();
        self.lower_class_declaration_with_name(&class_name, class)
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

        // For error subclasses with no explicit constructor, add an implicit message param.
        let builtin_error_names = ["Error", "TypeError", "RangeError", "ReferenceError", "SyntaxError"];
        let implicit_error_param = constructor.is_none()
            && parent_name.as_deref().map(|n| builtin_error_names.contains(&n)).unwrap_or(false);
        let n_params = constructor.map_or(0, |c| c.value.params.items.len())
            + if implicit_error_param { 1 } else { 0 };
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
        // For implicit error param (no explicit constructor, extends Error), bind message arg.
        if implicit_error_param {
            let msg: Value<'_, '_> = entry.argument(0)?.into();
            scope.insert("__implicit_error_msg".to_string(), msg);
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
        // - Parent is a built-in error type: call ts_error_new(message) using super() args.
        // - Parent + explicit super(): call parent ctor with those args.
        // - Parent + no super():    call parent ctor with no args.
        let builtin_error_parents = ["Error", "TypeError", "RangeError", "ReferenceError", "SyntaxError"];
        let parent_is_builtin_error = parent_name.as_deref()
            .map(|n| builtin_error_parents.contains(&n))
            .unwrap_or(false);

        let this_val: Value<'_, '_> = if parent_is_builtin_error {
            // For built-in error parents: create via ts_error_new(first_super_arg).
            let undef_i64: Value<'_, '_> = current.append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(i64_type, 0x7FF8_0000_0000_0000u64 as i64).into(),
                self.loc,
            )).result(0)?.into();
            // Use implicit error message arg if available (no explicit constructor)
            let msg_arg = if implicit_error_param {
                scope.get("__implicit_error_msg").copied().unwrap_or(undef_i64)
            } else if let Some(idx) = super_call_index {
                let ctor_body = constructor.unwrap().value.body.as_ref().unwrap();
                if let Statement::ExpressionStatement(es) = &ctor_body.statements[idx] {
                    if let Expression::CallExpression(call) = &es.expression {
                        if let Some(first_arg) = call.arguments.first() {
                            if let Some(expr) = first_arg.as_expression() {
                                let (v_opt, nb) = self.lower_expression(expr, current, &region, &mut scope)?;
                                current = nb;
                                v_opt.map(|v| self.ensure_i64(v, current)).transpose()?.unwrap_or(undef_i64)
                            } else { undef_i64 }
                        } else { undef_i64 }
                    } else { undef_i64 }
                } else { undef_i64 }
            } else { undef_i64 };
            let err_val: Value<'_, '_> = current.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_error_new"),
                &[msg_arg],
                &[i64_type],
                self.loc,
            )).result(0)?.into();
            current.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[msg_arg], &[], self.loc,
            ));
            err_val
        } else if let Some(ref parent_ctor) = parent_ctor_name {
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

            // If the parent constructor expects more args than we collected, pad with UNDEFINED.
            let parent_arity = parent_name.as_deref()
                .and_then(|n| self.classes.get(n))
                .map(|sig| sig.constructor_arity)
                .unwrap_or(0);
            while call_args.len() < parent_arity {
                let undef: Value<'_, '_> = current.append_operation(arith::constant(
                    self.ctx,
                    IntegerAttribute::new(i64_type, 0x7FF8_0000_0000_0000u64 as i64).into(),
                    self.loc,
                )).result(0)?.into();
                call_args.push(undef);
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
                    oxc_ast::ast::PropertyKey::PrivateIdentifier(id) => {
                        format!("__priv_{}", id.name.as_str())
                    }
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

        // Store each class method as a TsFunction property on `this`.
        // This enables dynamic method dispatch (e.g. `router.add(...)`) when the
        // class type isn't known statically at the call site.
        let ptr_type = melior::dialect::llvm::r#type::pointer(self.ctx, 0);
        let i32_type = self.i32_type();
        for elem in &class.body.body {
            let ClassElement::MethodDefinition(method) = elem else { continue };
            if method.value.body.is_none() { continue; }
            // Only expose public instance methods (not static, not getters/setters, not private)
            use oxc_ast::ast::MethodDefinitionKind;
            if method.r#static { continue; }
            if !matches!(method.kind, MethodDefinitionKind::Method) { continue; }
            let oxc_ast::ast::PropertyKey::StaticIdentifier(key_id) = &method.key else { continue };
            let method_name = key_id.name.to_string();
            let func_name = format!("__class_{}_{}", class_name, method_name);
            let n_params = method.value.params.items.len();
            let has_rest = method.value.params.rest.is_some();
            let all_mlir_params = 1 + n_params + if has_rest { 1 } else { 0 }; // +1 for `this`
            // arity for TsFunction does NOT include `this` (which is the first MLIR param)
            let arity = n_params as i64;
            let func_type_val = melior::ir::r#type::FunctionType::new(
                self.ctx,
                &vec![i64_type; all_mlir_params],
                &[i64_type],
            ).into();
            let fn_ref: Value<'_, '_> = current.append_operation(
                melior::ir::operation::OperationBuilder::new("func.constant", self.loc)
                    .add_attributes(&[(
                        melior::ir::Identifier::new(self.ctx, "value"),
                        FlatSymbolRefAttribute::new(self.ctx, &func_name).into(),
                    )])
                    .add_results(&[func_type_val])
                    .build()?,
            ).result(0)?.into();
            let fn_ptr_val: Value<'_, '_> = current.append_operation(
                melior::ir::operation::OperationBuilder::new("builtin.unrealized_conversion_cast", self.loc)
                    .add_operands(&[fn_ref])
                    .add_results(&[ptr_type])
                    .build()?,
            ).result(0)?.into();
            let arity_val: Value<'_, '_> = current.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i32_type, arity).into(), self.loc,
            )).result(0)?.into();
            let fn_val: Value<'_, '_> = current.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_func_new_this"),
                &[fn_ptr_val, arity_val], &[i64_type], self.loc,
            )).result(0)?.into();
            let key_ptr = self.get_string_ptr(&method_name, current)?;
            let this_i64 = self.ensure_i64(scope["this"], current)?;
            current.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
                &[this_i64, key_ptr, fn_val], &[], self.loc,
            ));
            // ts_obj_set retains fn_val (refcount now ≥ 2).
            // Apply method decorators before releasing our reference to fn_val.
            if !method.decorators.is_empty() {
                // Build descriptor = { value: fn_val }
                let descriptor: Value<'_, '_> = current.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_new"),
                    &[], &[i64_type], self.loc,
                )).result(0)?.into();
                let value_key = self.get_string_ptr("value", current)?;
                current.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
                    &[descriptor, value_key, fn_val], &[], self.loc,
                ));
                // Create a TsString for the method name.
                let name_c = self.get_string_ptr(&method_name, current)?;
                let name_str: Value<'_, '_> = current.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_string_new"),
                    &[name_c], &[i64_type], self.loc,
                )).result(0)?.into();
                // Apply decorators in reverse order (bottom decorator runs first).
                for dec in method.decorators.iter().rev() {
                    let (dec_opt, nb) = self.lower_expression(&dec.expression, current, &region, &mut scope)?;
                    current = nb;
                    if let Some(dec_fn) = dec_opt {
                        let dec_fn_i64 = self.ensure_i64(dec_fn, current)?;
                        // Call decorator(this, "methodName", descriptor)
                        let res: Value<'_, '_> = current.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_func_call3"),
                            &[dec_fn_i64, this_i64, name_str, descriptor],
                            &[i64_type], self.loc,
                        )).result(0)?.into();
                        current.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                            &[dec_fn_i64], &[], self.loc,
                        ));
                        current.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                            &[res], &[], self.loc,
                        ));
                    }
                }
                current.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[descriptor], &[], self.loc,
                ));
                current.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[name_str], &[], self.loc,
                ));
            }
            // Release our original reference to fn_val (this still retains it).
            current.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[fn_val], &[], self.loc,
            ));
        }

        // Lower constructor body (skip the super() call we already processed).
        let saved_fn_return_type_cls = self.fn_return_type;
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
        self.fn_return_type = saved_fn_return_type_cls;

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
            has_rest: false,
            has_this_param: false,
        });
        Ok(())
    }

    // ── Instance methods ──────────────────────────────────────────────────

    fn lower_class_method(&mut self, class_name: &str, method: &MethodDefinition<'_>) -> Result<()> {
        // Resolve method name: public or private (#name → __priv_name)
        let name = match &method.key {
            oxc_ast::ast::PropertyKey::StaticIdentifier(id) => id.name.to_string(),
            oxc_ast::ast::PropertyKey::PrivateIdentifier(id) => {
                format!("__priv_{}", id.name.as_str())
            }
            _ => match method.key.static_name() {
                Some(n) => n.to_string(),
                None => return Ok(()),
            },
        };
        let func_name = format!("__class_{}_{}", class_name, name);
        let i64_type  = self.i64_type();

        let has_rest = method.value.params.rest.is_some();
        let n_params = method.value.params.items.len();
        let all_params = 1 + n_params + if has_rest { 1 } else { 0 };
        let param_specs: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            (0..all_params).map(|_| (i64_type, self.loc)).collect();
        let func_type = FunctionType::new(self.ctx, &vec![i64_type; all_params], &[i64_type]);

        let region = Region::new();
        let entry  = region.append_block(Block::new(&param_specs));

        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();
        scope.insert("this".to_string(), entry.argument(0)?.into());
        self.var_class_types.insert("this".to_string(), class_name.to_string());
        let mut param_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        param_names.insert("this".to_string());
        for (i, param) in method.value.params.items.iter().enumerate() {
            if let BindingPattern::BindingIdentifier(id) = &param.pattern {
                scope.insert(id.name.to_string(), entry.argument(1 + i)?.into());
                param_names.insert(id.name.to_string());
            }
        }
        // Bind rest parameter: it receives the last MLIR argument (a pre-built TsArray).
        if let Some(rest) = &method.value.params.rest {
            if let BindingPattern::BindingIdentifier(id) = &rest.rest.argument {
                let rest_arg_idx = 1 + n_params; // after `this` + regular params
                let rest_val: Value<'_, '_> = entry.argument(rest_arg_idx)?.into();
                let rname = id.name.to_string();
                scope.insert(rname.clone(), rest_val);
                param_names.insert(rname);
            }
        }
        let saved_fn_params = std::mem::replace(&mut self.current_fn_params, param_names);

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

        let saved_fn_return_type_cls = self.fn_return_type;
        let saved_is_async = self.is_async;
        self.fn_return_type = i64_type;
        self.is_async = method.value.r#async;
        if let Some(body) = &method.value.body {
            let saved_cell_vars = std::mem::replace(
                &mut self.cell_vars,
                crate::lowering::expressions::compute_cell_vars_for_body(&body.statements),
            );
            let saved_cell_captures = std::mem::replace(&mut self.cell_captures, std::collections::HashSet::new());
            for stmt in &body.statements {
                let (val, next) = self.lower_statement(stmt, current, &region, &mut scope, &[])?;
                current = next;
                if let Some(v) = val { result = v; }
            }
            self.cell_vars = saved_cell_vars;
            self.cell_captures = saved_cell_captures;
        }
        self.fn_return_type = saved_fn_return_type_cls;

        if current.terminator().is_none() {
            if self.is_async {
                let result_i64 = self.ensure_i64(result, current)?;
                let promise: melior::ir::Value<'_, '_> = current.append_operation(melior::dialect::func::call(
                    self.ctx,
                    melior::ir::attribute::FlatSymbolRefAttribute::new(self.ctx, "ts_promise_resolve"),
                    &[result_i64],
                    &[i64_type],
                    self.loc,
                )).result(0)?.into();
                current.append_operation(melior::dialect::func::call(
                    self.ctx,
                    melior::ir::attribute::FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[result_i64],
                    &[],
                    self.loc,
                ));
                current.append_operation(melior::dialect::func::r#return(&[promise], self.loc));
            } else {
                let result_i64 = self.ensure_i64(result, current)?;
                current.append_operation(melior::dialect::func::r#return(&[result_i64], self.loc));
            }
        }
        self.is_async = saved_is_async;
        self.current_fn_params = saved_fn_params;

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
            has_rest,
            has_this_param: false,
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
        let mut param_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        param_names.insert("this".to_string());
        let saved_fn_params = std::mem::replace(&mut self.current_fn_params, param_names);

        let zero_i64: Value<'_, '_> = entry
            .append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64_type, 0).into(), self.loc,
            )).result(0)?.into();

        let mut result = zero_i64;
        let mut current = entry;

        let saved_fn_return_type_cls = self.fn_return_type;
        self.fn_return_type = i64_type;
        if let Some(body) = &method.value.body {
            for stmt in &body.statements {
                let (val, next) = self.lower_statement(stmt, current, &region, &mut scope, &[])?;
                current = next;
                if let Some(v) = val { result = v; }
            }
        }
        self.fn_return_type = saved_fn_return_type_cls;
        self.current_fn_params = saved_fn_params;

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
            has_rest: false,
            has_this_param: false,
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

        let saved_fn_return_type_cls = self.fn_return_type;
        self.fn_return_type = i64_type;
        if let Some(body) = &method.value.body {
            for stmt in &body.statements {
                let (_, next) = self.lower_statement(stmt, current, &region, &mut scope, &[])?;
                current = next;
            }
        }
        self.fn_return_type = saved_fn_return_type_cls;

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
            has_rest: false,
            has_this_param: false,
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

        let saved_fn_return_type_cls = self.fn_return_type;
        let saved_is_async_static = self.is_async;
        self.fn_return_type = i64_type;
        self.is_async = method.value.r#async;
        if let Some(body) = &method.value.body {
            let saved_cell_vars_s = std::mem::replace(
                &mut self.cell_vars,
                crate::lowering::expressions::compute_cell_vars_for_body(&body.statements),
            );
            let saved_cell_captures_s = std::mem::replace(&mut self.cell_captures, std::collections::HashSet::new());
            for stmt in &body.statements {
                let (val, next) = self.lower_statement(stmt, current, &region, &mut scope, &[])?;
                current = next;
                if let Some(v) = val { result = v; }
            }
            self.cell_vars = saved_cell_vars_s;
            self.cell_captures = saved_cell_captures_s;
        }
        self.fn_return_type = saved_fn_return_type_cls;

        if current.terminator().is_none() {
            if self.is_async {
                let result_i64 = self.ensure_i64(result, current)?;
                let promise: melior::ir::Value<'_, '_> = current.append_operation(melior::dialect::func::call(
                    self.ctx,
                    melior::ir::attribute::FlatSymbolRefAttribute::new(self.ctx, "ts_promise_resolve"),
                    &[result_i64],
                    &[i64_type],
                    self.loc,
                )).result(0)?.into();
                current.append_operation(melior::dialect::func::call(
                    self.ctx,
                    melior::ir::attribute::FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[result_i64],
                    &[],
                    self.loc,
                ));
                current.append_operation(melior::dialect::func::r#return(&[promise], self.loc));
            } else {
                let result_i64 = self.ensure_i64(result, current)?;
                current.append_operation(func::r#return(&[result_i64], self.loc));
            }
        }
        self.is_async = saved_is_async_static;

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
            has_rest: false,
            has_this_param: false,
        });
        Ok(())
    }
}
