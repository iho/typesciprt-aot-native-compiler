use super::*;

impl<'c, 'm> Lowerer<'c, 'm> {
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
            "console", "process", "setTimeout", "clearTimeout", "setInterval", "clearInterval",
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
                    self.destructure_array_pattern_into_scope(arr_pat, arg_i64, arrow_entry, &arrow_region, &mut arrow_scope)?;
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

        // Collect param names for ARC: lower_return_statement skips these.
        let mut arrow_param_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        for param in params.iter() {
            match &param.pattern {
                oxc_ast::ast::BindingPattern::BindingIdentifier(id) => {
                    arrow_param_names.insert(id.name.to_string());
                }
                oxc_ast::ast::BindingPattern::ArrayPattern(arr_pat) => {
                    // Destructured params: the extracted bindings are locals owned by this fn.
                    // The raw arg (arg_i64) is still borrowed — but we destructured it, so we
                    // need to release it. Mark nothing here; the caller never sees these names.
                    let _ = arr_pat;
                }
                oxc_ast::ast::BindingPattern::ObjectPattern(obj_pat) => {
                    // Destructured object param: extracted bindings are locals (owned via ts_obj_get).
                    // The original arg_i64 was used to extract and is NOT in scope by name — skip.
                    let _ = obj_pat;
                }
                _ => {}
            }
        }
        if let Some(rn) = rest_param_name {
            arrow_param_names.insert(rn.to_string());
        }

        let saved_return_type = self.fn_return_type;
        let saved_is_async = self.is_async;
        let saved_fn_params = std::mem::replace(&mut self.current_fn_params, arrow_param_names);
        let saved_env_indices = std::mem::replace(
            &mut self.closure_env_indices,
            if has_captures {
                free_vars.iter().cloned().enumerate().map(|(i, v)| (v, i)).collect()
            } else {
                HashMap::new()
            },
        );
        // Compute cell_vars for this closure body (locally-declared vars mutated in nested closures).
        // Also compute cell_captures: free vars that are cells in the outer scope.
        let inner_cell_captures: NameSet = free_vars.iter()
            .filter(|v| self.is_cell_var(v))
            .cloned()
            .collect();
        let inner_cell_vars = compute_cell_vars_for_body(body_stmts);
        let saved_cell_vars = std::mem::replace(&mut self.cell_vars, inner_cell_vars);
        let saved_cell_captures = std::mem::replace(&mut self.cell_captures, inner_cell_captures);
        let mut inner_scalar_vars = compute_scalar_vars_for_body(body_stmts);
        inner_scalar_vars.retain(|v| !self.cell_vars.contains(v));
        // Arrow/function parameters with scalar type annotations are also scalar.
        for param in params.iter() {
            if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &param.pattern {
                if let Some(ann) = &param.type_annotation {
                    if crate::lowering::ts_type_is_scalar(&ann.type_annotation) {
                        inner_scalar_vars.insert(id.name.to_string());
                    }
                }
            }
        }
        let saved_scalar_vars_arrow = std::mem::replace(&mut self.scalar_vars, inner_scalar_vars);
        let mut inner_nea = compute_non_escaping_allocs(body_stmts);
        inner_nea.retain(|v| !self.cell_vars.contains(v));
        let saved_non_escaping_arrow = std::mem::replace(&mut self.non_escaping_allocs, inner_nea);
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
        // Track whether this is an expression-body arrow (=> expr) vs block-body ({ ... }).
        // Expression-body arrows return the expression's value (no release before return).
        // Block-body arrows always return UNDEFINED unless there's an explicit `return` stmt.
        let is_expr_body = block_body.is_none() && expr_body.is_some();

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

            // ── Hoist pass: lower all inner FunctionDeclarations first ───────────
            // JavaScript hoists function declarations to the top of the enclosing
            // function scope. We replicate that by creating closures for all inner
            // named function declarations before processing any other statements.
            // This ensures calls like `return dispatch(0); function dispatch(i){…}`
            // work correctly even though `dispatch` is declared after its first use.
            for stmt in &body.statements {
                let oxc_ast::ast::Statement::FunctionDeclaration(inner_fn) = stmt else { continue };
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
            }

            // ── Main pass: process non-FunctionDeclaration statements ────────────
            for stmt in &body.statements {
                if matches!(stmt, oxc_ast::ast::Statement::FunctionDeclaration(_)) {
                    // Already handled in the hoist pass above.
                    continue;
                } else if is_expr_body {
                    // Expression-body arrow (=> expr): lower the expression directly without
                    // releasing it so the owned reference is preserved for the return value.
                    if let oxc_ast::ast::Statement::ExpressionStatement(es) = stmt {
                        let (val_opt, nb) = self.lower_expression(&es.expression, current_block, &arrow_region, &mut arrow_scope)?;
                        current_block = nb;
                        if let Some(val) = val_opt {
                            result_val = Some(self.ensure_i64(val, current_block)?);
                        }
                    } else {
                        // Unexpected non-expression statement in expr body; treat like block body.
                        let (_, nb) = self.lower_statement(stmt, current_block, &arrow_region, &mut arrow_scope, &[])?;
                        current_block = nb;
                    }
                } else {
                    // Block-body arrow: process statement but DO NOT propagate result_val.
                    // Block-body arrows always return UNDEFINED unless there is an explicit
                    // `return` statement (which calls func.return directly).
                    let (_, nb) = self.lower_statement(stmt, current_block, &arrow_region, &mut arrow_scope, &[])?;
                    current_block = nb;
                }
            }
        }

        self.fn_return_type = saved_return_type;
        self.is_async = saved_is_async;
        self.current_fn_params = saved_fn_params;
        self.closure_env_indices = saved_env_indices;
        self.cell_vars = saved_cell_vars;
        self.cell_captures = saved_cell_captures;
        self.scalar_vars = saved_scalar_vars_arrow;
        self.non_escaping_allocs = saved_non_escaping_arrow;

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
            &[(
                Identifier::new(self.ctx, "sym_visibility"),
                StringAttribute::new(self.ctx, "private").into(),
            )],
            self.loc,
        );
        self.module.body().append_operation(op);

        // Register so dynamic calls know the signature.
        self.funcs.insert(name.clone(), FuncSig {
            param_types: vec![i64_type; total_mlir_params],
            return_type: Some(i64_type),
            has_rest: false,
            has_this_param: false,
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
                    // ts_arr_set retains internally; no explicit retain needed here
                    v_i64
                } else if self.funcs.contains_key(var_name) {
                    // Module-level function not yet in scope: create a TsFunction wrapper.
                    let sig = self.funcs[var_name].clone();
                    let this_offset = if sig.has_this_param { 1 } else { 0 };
                    let arity = (sig.param_types.len() - this_offset) as i64;
                    let ptr_type = melior::dialect::llvm::r#type::pointer(self.ctx, 0);
                    let func_type_val = melior::ir::r#type::FunctionType::new(
                        self.ctx, &sig.param_types, &[i64_type],
                    ).into();
                    let fn_ref: Value<'c, 'b> = block.append_operation(
                        melior::ir::operation::OperationBuilder::new("func.constant", self.loc)
                            .add_attributes(&[(
                                melior::ir::Identifier::new(self.ctx, "value"),
                                FlatSymbolRefAttribute::new(self.ctx, var_name).into(),
                            )])
                            .add_results(&[func_type_val])
                            .build()?,
                    ).result(0)?.into();
                    let fn_ptr_val: Value<'c, 'b> = block.append_operation(
                        melior::ir::operation::OperationBuilder::new("builtin.unrealized_conversion_cast", self.loc)
                            .add_operands(&[fn_ref])
                            .add_results(&[ptr_type])
                            .build()?,
                    ).result(0)?.into();
                    let arity_val2: Value<'c, 'b> = block.append_operation(arith::constant(
                        self.ctx, IntegerAttribute::new(i32_type, arity).into(), self.loc,
                    )).result(0)?.into();
                    let ctor = if sig.has_this_param { "ts_func_new_this" } else { "ts_func_new" };
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, ctor),
                        &[fn_ptr_val, arity_val2], &[i64_type], self.loc,
                    )).result(0)?.into()
                } else {
                    // Not found in scope or funcs: use undefined
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

            let closure_fn_name = if has_rest { "ts_closure_new_rest" } else { "ts_closure_new" };
            let result = block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, closure_fn_name),
                &[fn_ptr, arity_val, env_arr],
                &[i64_type],
                self.loc,
            )).result(0)?.into();
            // ts_closure_new(_rest) retains env; release our temporary ref
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
    /// Returns the (possibly updated) current block, since default-value handling may create new blocks.
    pub(super) fn destructure_array_pattern_into_scope<'b>(
        &mut self,
        arr_pat: &oxc_ast::ast::ArrayPattern<'_>,
        arr_val: Value<'c, 'b>,
        mut block: BlockRef<'c, 'b>,
        region: &Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<BlockRef<'c, 'b>> {
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
                    block = self.destructure_array_pattern_into_scope(inner, elem_val, block, region, scope)?;
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
                        // Apply default: if elem is undefined, evaluate the default expression.
                        let is_undef: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_undefined"),
                            &[elem_val], &[i32t], self.loc,
                        )).result(0)?.into();
                        let is_undef_i1 = self.ensure_i1(is_undef, block)?;
                        let merge_block = region.append_block(Block::new(&[(i64t, self.loc)]));
                        let default_block = region.append_block(Block::new(&[]));
                        block.append_operation(cf::cond_br(
                            self.ctx, is_undef_i1, &default_block, &merge_block, &[], &[elem_val], self.loc,
                        ));
                        let mut def_scope = scope.clone();
                        let (def_opt, post_def) = self.lower_expression(&ap.right, default_block, region, &mut def_scope)?;
                        let def_val = def_opt.ok_or_else(|| anyhow::anyhow!("destructuring default: no value"))?;
                        let def_i64 = self.ensure_i64(def_val, post_def)?;
                        post_def.append_operation(cf::br(&merge_block, &[def_i64], self.loc));
                        block = merge_block;
                        scope.insert(id.name.to_string(), merge_block.argument(0)?.into());
                    }
                }
            }
        }
        Ok(block)
    }

}
