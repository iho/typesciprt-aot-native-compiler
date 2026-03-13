use super::*;

impl<'c, 'm> Lowerer<'c, 'm> {

    // ── Implicit main function ────────────────────────────────────────────

    pub(super) fn lower_main_function(&mut self, program: &Program<'_>) -> Result<()> {
        let i32_type = self.i32_type();
        let main_type = FunctionType::new(self.ctx, &[], &[i32_type]);

        let region = Region::new();
        let entry = region.append_block(Block::new(&[]));
        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();

        let mut result_value: Value<'_, '_> = entry
            .append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(i32_type, 0).into(),
                self.loc,
            ))
            .result(0)?
            .into();
        let mut current_block = entry;

        for stmt in &program.body {
            // Function declarations are emitted separately; skip here.
            if matches!(stmt, Statement::FunctionDeclaration(_)) {
                continue;
            }
            let (val, next) = self.lower_statement(stmt, current_block, &region, &mut scope, &[])?;
            current_block = next;
            if let Some(v) = val {
                result_value = v;
            }
        }

        // ARC: Release all variables in the main scope before returning.
        for (_, v) in &scope {
            let v_i64 = self.ensure_i64(*v, current_block)?;
            current_block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[v_i64],
                &[],
                self.loc,
            ));
        }

        self.terminate_with_return(current_block, result_value)?;

        let op = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "main"),
            TypeAttribute::new(main_type.into()),
            region,
            &[(
                Identifier::new(self.ctx, "sym_visibility"),
                StringAttribute::new(self.ctx, "public").into(),
            )],
            self.loc,
        );
        self.module.body().append_operation(op);
        Ok(())
    }

    // ── Module-level const arrow hoisting ────────────────────────────────

    /// Lower module-level `const name = (params) => body` declarations as top-level MLIR functions.
    /// This is called after `collect_function_signatures` so `self.funcs` already has all sigs.
    pub(super) fn lower_module_const_functions(&mut self, program: &Program<'_>) -> Result<()> {
        use oxc_ast::ast::ExportNamedDeclaration;
        for stmt in &program.body {
            let vd_opt: Option<&oxc_ast::ast::VariableDeclaration<'_>> = match stmt {
                Statement::VariableDeclaration(vd) => Some(vd),
                Statement::ExportNamedDeclaration(exp) => {
                    if let Some(Declaration::VariableDeclaration(vd)) = &exp.declaration {
                        Some(vd)
                    } else { None }
                }
                _ => None,
            };
            let Some(vd) = vd_opt else { continue };
            for decl in &vd.declarations {
                let name = match &decl.id {
                    BindingPattern::BindingIdentifier(id) => id.name.to_string(),
                    _ => continue,
                };
                // Only process if we registered a sig for this name (arrow or function expr).
                if !self.funcs.contains_key(&name) { continue; }
                let init = match &decl.init { Some(e) => e, None => continue };
                let inner = Lowerer::strip_ts_casts(init);
                match inner {
                    Expression::ArrowFunctionExpression(arrow) => {
                        let params: Vec<&oxc_ast::ast::FormalParameter<'_>> =
                            arrow.params.items.iter().collect();
                        let rest_name = arrow.params.rest.as_ref()
                            .and_then(|r| if let BindingPattern::BindingIdentifier(id) = &r.rest.argument { Some(id.name.as_str()) } else { None });
                        self.lower_named_function(&name, &params, rest_name, Some(&arrow.body), None)?;
                    }
                    Expression::FunctionExpression(func_expr) => {
                        let params: Vec<&oxc_ast::ast::FormalParameter<'_>> =
                            func_expr.params.items.iter().collect();
                        let rest_name = func_expr.params.rest.as_ref()
                            .and_then(|r| if let BindingPattern::BindingIdentifier(id) = &r.rest.argument { Some(id.name.as_str()) } else { None });
                        let body = func_expr.body.as_deref();
                        self.lower_named_function(&name, &params, rest_name, body, None)?;
                    }
                    _ => {} // alias or non-function const — skip
                }
            }
        }
        Ok(())
    }

    // ── Statement lowering ────────────────────────────────────────────────

    pub(super) fn lower_statement<'b>(
        &mut self,
        stmt: &Statement<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
        loops: &[(BlockRef<'c, 'b>, BlockRef<'c, 'b>, Vec<String>)],
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        match stmt {
            Statement::ExpressionStatement(es) => {
                let (val_opt, nb) = self.lower_expression(&es.expression, block, region, scope)?;
                if let Some(val) = val_opt {
                    let val_i64 = self.ensure_i64(val, nb)?;
                    nb.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[val_i64],
                        &[],
                        self.loc,
                    ));
                    // Return the (released) value so main() can use it as exit code.
                    // For integers ts_release_val is a no-op, so the SSA value remains valid.
                    return Ok((Some(val_i64), nb));
                }
                Ok((None, nb))
            }
            Statement::VariableDeclaration(vd) => {
                self.lower_variable_declaration(vd, block, region, scope)
            }
            Statement::FunctionDeclaration(_) => Ok((None, block)), // already handled
            Statement::ReturnStatement(ret) => {
                self.lower_return_statement(ret, block, region, scope)
            }
            Statement::IfStatement(if_stmt) => {
                self.lower_if_statement(if_stmt, block, region, scope, loops)
            }
            Statement::WhileStatement(w) => {
                self.lower_while_statement(w, block, region, scope, loops)
            }
            Statement::ForStatement(f) => {
                self.lower_for_statement(f, block, region, scope, loops)
            }
            Statement::BlockStatement(bs) => {
                let mut cur = block;
                let mut last = None;
                let mut inner = scope.clone();
                for s in &bs.body {
                    let (v, nb) = self.lower_statement(s, cur, region, &mut inner, loops)?;
                    cur = nb;
                    if let Some(v) = v { last = Some(v); }
                }
                
                // ARC: Release locals.
                for (k, v) in &inner {
                    if !scope.contains_key(k) {
                        let v_i64 = self.ensure_i64(*v, cur)?;
                        cur.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                            &[v_i64],
                            &[],
                            self.loc,
                        ));
                    } else {
                        scope.insert(k.clone(), *v);
                    }
                }
                Ok((last, cur))
            }

            Statement::BreakStatement(_) => {
                if let Some((_, exit_block, scope_keys)) = loops.last() {
                    let vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
                    self.terminate_with_br(block, exit_block, &vals);
                    let dead = region.append_block(Block::new(&[]));
                    Ok((None, dead))
                } else {
                    bail!("break statement outside of loop");
                }
            }
            Statement::ContinueStatement(_) => {
                if let Some((header_block, _, scope_keys)) = loops.last() {
                    let vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
                    self.terminate_with_br(block, header_block, &vals);
                    let dead = region.append_block(Block::new(&[]));
                    Ok((None, dead))
                } else {
                    bail!("continue statement outside of loop");
                }
            }
            Statement::ThrowStatement(throw) => {
                self.lower_throw_statement(throw, block, region, scope)
            }
            Statement::TryStatement(try_stmt) => {
                self.lower_try_statement(try_stmt, block, region, scope, loops)
            }
            Statement::ForOfStatement(for_of) => {
                self.lower_for_of_statement(for_of, block, region, scope, loops)
            }
            Statement::ForInStatement(for_in) => {
                self.lower_for_in_statement(for_in, block, region, scope, loops)
            }
            Statement::ClassDeclaration(_) => {
                Ok((None, block)) // Already lowered in the dedicated pass.
            }
            Statement::TSInterfaceDeclaration(_)
            | Statement::TSTypeAliasDeclaration(_)
            | Statement::TSModuleDeclaration(_) => {
                Ok((None, block)) // pure type information, no runtime representation
            }
            Statement::TSEnumDeclaration(enum_decl) => {
                self.lower_enum_declaration(enum_decl, block, region, scope)
            }
            Statement::ExportNamedDeclaration(export) => {
                if let Some(decl) = &export.declaration {
                    match decl {
                        Declaration::FunctionDeclaration(_) | Declaration::ClassDeclaration(_) => {
                            Ok((None, block)) // already handled in hoisting passes
                        }
                        Declaration::VariableDeclaration(vd) => {
                            self.lower_variable_declaration(vd, block, region, scope)
                        }
                        Declaration::TSEnumDeclaration(enum_decl) => {
                            self.lower_enum_declaration(enum_decl, block, region, scope)
                        }
                        _ => Ok((None, block)),
                    }
                } else {
                    Ok((None, block))
                }
            }
            Statement::ExportDefaultDeclaration(export) => {
                use oxc_ast::ast::ExportDefaultDeclarationKind;
                match &export.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(_)
                    | ExportDefaultDeclarationKind::ClassDeclaration(_) => {
                        Ok((None, block)) // already handled in hoisting passes
                    }
                    _ => Ok((None, block)),
                }
            }
            Statement::ImportDeclaration(_) => {
                Ok((None, block)) // handled in the import pre-pass in lower_program
            }
            _ => {
                tracing::debug!("skipping unimplemented statement kind");
                Ok((None, block))
            }
        }
    }

    // ── Variable declarations ─────────────────────────────────────────────

    pub(super) fn lower_variable_declaration<'b>(
        &mut self,
        var_decl: &VariableDeclaration<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        for declarator in &var_decl.declarations {
            match &declarator.id {
                BindingPattern::BindingIdentifier(b) => {
                    let name = b.name.to_string();
                    if declarator.init.is_none() {
                        // `let x` with no initializer — initialize to undefined.
                        let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                            self.ctx,
                            IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(),
                            self.loc,
                        )).result(0)?.into();
                        scope.insert(name.clone(), undef);
                    }
                    if let Some(init) = &declarator.init {
                        // Type inference: record class name for `let x = new Foo()`.
                        if let Expression::NewExpression(new_expr) = init {
                            if let Expression::Identifier(id) = &new_expr.callee {
                                self.var_class_types.insert(name.clone(), id.name.to_string());
                            }
                        }
                        let (val_opt, nb) = self.lower_expression(init, block, region, scope)?;
                        block = nb;
                        if let Some(val) = val_opt {
                            scope.insert(name.clone(), val);
                            // If this is a module-level global, also store it in the runtime global map.
                            if self.module_global_names.contains(&name) {
                                let val_i64 = self.ensure_i64(val, block)?;
                                let key_ptr = self.get_string_ptr(&name, block)?;
                                block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_set_module_global"),
                                    &[key_ptr, val_i64], &[], self.loc,
                                ));
                            }
                        }
                    }
                }

                BindingPattern::ObjectPattern(obj_pat) => {
                    // const { a, b, ...rest } = expr  →  evaluate expr once, then ts_obj_get each key
                    let Some(init) = &declarator.init else { continue };
                    let (val_opt, nb) = self.lower_expression(init, block, region, scope)?;
                    block = nb;
                    let Some(obj_val) = val_opt else { continue };
                    let obj_i64 = self.ensure_i64(obj_val, block)?;

                    let mut extracted_keys: Vec<String> = Vec::new();
                    for prop in &obj_pat.properties {
                        // Determine the property key string (only static keys for now).
                        let key_str = match prop.key.static_name() {
                            Some(n) => n.into_owned(),
                            None => {
                                tracing::debug!("skipping computed destructuring key");
                                continue;
                            }
                        };
                        // Determine binding name and optional default initializer.
                        let (var_name, default_init) = match &prop.value {
                            BindingPattern::BindingIdentifier(id) => (id.name.to_string(), None),
                            BindingPattern::AssignmentPattern(ap) => {
                                if let BindingPattern::BindingIdentifier(id) = &ap.left {
                                    (id.name.to_string(), Some(&ap.right))
                                } else {
                                    tracing::debug!("skipping nested destructuring pattern");
                                    continue;
                                }
                            }
                            _ => {
                                tracing::debug!("skipping nested destructuring pattern");
                                continue;
                            }
                        };
                        let key_ptr = self.get_string_ptr(&key_str, block)?;
                        let field_val: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                            &[obj_i64, key_ptr],
                            &[self.i64_type()],
                            self.loc,
                        )).result(0)?.into();
                        // Handle default value: if field is undefined, use the initializer.
                        let final_val = if let Some(default_expr) = default_init {
                            let i64t = self.i64_type();
                            let i32t = self.i32_type();
                            let is_undef: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_undefined"),
                                &[field_val], &[i32t], self.loc,
                            )).result(0)?.into();
                            let is_undef_i1 = self.ensure_i1(is_undef, block)?;
                            let merge_block = region.append_block(Block::new(&[(i64t, self.loc)]));
                            let default_block = region.append_block(Block::new(&[]));
                            block.append_operation(cf::cond_br(
                                self.ctx, is_undef_i1, &default_block, &merge_block, &[], &[field_val], self.loc,
                            ));
                            let mut def_scope = scope.clone();
                            let (def_opt, post_def) = self.lower_expression(default_expr, default_block, region, &mut def_scope)?;
                            let def_val = def_opt.ok_or_else(|| anyhow::anyhow!("destructuring default: no value"))?;
                            let def_i64 = self.ensure_i64(def_val, post_def)?;
                            post_def.append_operation(cf::br(&merge_block, &[def_i64], self.loc));
                            block = merge_block;
                            merge_block.argument(0)?.into()
                        } else {
                            field_val
                        };
                        scope.insert(var_name, final_val);
                        extracted_keys.push(key_str);
                    }

                    // Handle rest element: `const { a, ...rest } = obj`
                    if let Some(rest_el) = &obj_pat.rest {
                        if let BindingPattern::BindingIdentifier(rest_id) = &rest_el.argument {
                            let rest_name = rest_id.name.to_string();
                            // Build a TsArray of excluded key strings.
                            let n_keys = extracted_keys.len() as i32;
                            let i32_type = self.i32_type();
                            let n_val: Value<'c, 'b> = block.append_operation(arith::constant(
                                self.ctx,
                                IntegerAttribute::new(i32_type, n_keys as i64).into(),
                                self.loc,
                            )).result(0)?.into();
                            let keys_arr: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx,
                                FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
                                &[n_val],
                                &[self.i64_type()],
                                self.loc,
                            )).result(0)?.into();
                            for (idx, key_str) in extracted_keys.iter().enumerate() {
                                let key_ptr = self.get_string_ptr(key_str, block)?;
                                let key_val: Value<'c, 'b> = block.append_operation(func::call(
                                    self.ctx,
                                    FlatSymbolRefAttribute::new(self.ctx, "ts_string_new"),
                                    &[key_ptr],
                                    &[self.i64_type()],
                                    self.loc,
                                )).result(0)?.into();
                                let idx_val: Value<'c, 'b> = block.append_operation(arith::constant(
                                    self.ctx,
                                    IntegerAttribute::new(i32_type, idx as i64).into(),
                                    self.loc,
                                )).result(0)?.into();
                                block.append_operation(func::call(
                                    self.ctx,
                                    FlatSymbolRefAttribute::new(self.ctx, "ts_arr_set"),
                                    &[keys_arr, idx_val, key_val],
                                    &[],
                                    self.loc,
                                ));
                                block.append_operation(func::call(
                                    self.ctx,
                                    FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                    &[key_val], &[], self.loc,
                                ));
                            }
                            let rest_val: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx,
                                FlatSymbolRefAttribute::new(self.ctx, "ts_obj_rest"),
                                &[obj_i64, keys_arr],
                                &[self.i64_type()],
                                self.loc,
                            )).result(0)?.into();
                            block.append_operation(func::call(
                                self.ctx,
                                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                &[keys_arr], &[], self.loc,
                            ));
                            scope.insert(rest_name, rest_val);
                        }
                    }

                    // ARC: release the init expression value now that we have extracted all fields.
                    block.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[obj_i64], &[], self.loc,
                    ));
                }

                BindingPattern::ArrayPattern(arr_pat) => {
                    // const [x, y, ...rest] = expr  →  evaluate expr once, then ts_arr_get each index
                    let Some(init) = &declarator.init else { continue };
                    let (val_opt, nb) = self.lower_expression(init, block, region, scope)?;
                    block = nb;
                    let Some(arr_val) = val_opt else { continue };
                    let arr_i64 = self.ensure_i64(arr_val, block)?;

                    let mut extracted_count: i64 = 0;
                    for (i, elem) in arr_pat.elements.iter().enumerate() {
                        let Some(pat) = elem else {
                            extracted_count = i as i64 + 1;
                            continue;
                        }; // skip holes
                        let var_name = match pat {
                            BindingPattern::BindingIdentifier(id) => id.name.to_string(),
                            _ => {
                                tracing::debug!("skipping nested array destructuring");
                                extracted_count = i as i64 + 1;
                                continue;
                            }
                        };
                        let idx_val = self.lower_numeric_literal(i as i64, block)?;
                        let elem_val: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
                            &[arr_i64, idx_val],
                            &[self.i64_type()],
                            self.loc,
                        )).result(0)?.into();
                        scope.insert(var_name, elem_val);
                        extracted_count = i as i64 + 1;
                    }

                    // Handle rest element: `const [a, b, ...rest] = arr`
                    if let Some(rest_el) = &arr_pat.rest {
                        if let BindingPattern::BindingIdentifier(rest_id) = &rest_el.argument {
                            let rest_name = rest_id.name.to_string();
                            let i32_type = self.i32_type();
                            let start_val: Value<'c, 'b> = block.append_operation(arith::constant(
                                self.ctx,
                                IntegerAttribute::new(i32_type, extracted_count).into(),
                                self.loc,
                            )).result(0)?.into();
                            let rest_val: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx,
                                FlatSymbolRefAttribute::new(self.ctx, "ts_arr_rest"),
                                &[arr_i64, start_val],
                                &[self.i64_type()],
                                self.loc,
                            )).result(0)?.into();
                            scope.insert(rest_name, rest_val);
                        }
                    }

                    // ARC: release the init expression value.
                    block.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[arr_i64], &[], self.loc,
                    ));
                }

                _ => {
                    tracing::debug!("skipping unsupported binding pattern");
                }
            }
        }
        Ok((None, block))
    }

    // ── Return statement ──────────────────────────────────────────────────

    pub(super) fn lower_return_statement<'b>(
        &mut self,
        ret: &oxc_ast::ast::ReturnStatement<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let val = if let Some(arg) = &ret.argument {
            let (val_opt, nb) = self.lower_expression(arg, block, region, scope)?;
            block = nb;
            val_opt.ok_or_else(|| anyhow::anyhow!("return: expression produced no value"))?
        } else {
            // `return;` → return 0
            block.append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(self.i32_type(), 0).into(),
                self.loc,
            )).result(0)?.into()
        };

        // For async functions, wrap the return value in a resolved Promise.
        let val = if self.is_async {
            let val_i64 = self.ensure_i64(val, block)?;
            block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_promise_resolve"),
                &[val_i64],
                &[self.i64_type()],
                self.loc,
            )).result(0)?.into()
        } else {
            val
        };

        // ARC: Release all variables in the current scope before returning.
        // "__env" is the env array passed in from the caller and is not owned by this closure.
        for (name, v) in scope.iter() {
            if name == "__env" { continue; }
            let v_i64 = self.ensure_i64(*v, block)?;
            block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[v_i64],
                &[],
                self.loc,
            ));
        }

        self.terminate_with_return(block, val)?;

        // Create a dead block to absorb any unreachable code after this return.
        let dead = region.append_block(Block::new(&[]));
        Ok((None, dead))
    }

    // ── If / else  (phi-node merge) ───────────────────────────────────────

    pub(super) fn lower_if_statement<'b>(
        &mut self,
        if_stmt: &IfStatement<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
        loops: &[(BlockRef<'c, 'b>, BlockRef<'c, 'b>, Vec<String>)],
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let (cond_opt, nb) = self.lower_expression(&if_stmt.test, block, region, scope)?;
        block = nb;
        let cond_val = cond_opt.ok_or_else(|| anyhow::anyhow!("if condition must produce a value"))?;
        let cond_i1 = self.ensure_i1(cond_val, block)?;

        let scope_keys: Vec<String> = scope.keys().cloned().collect();
        // Normalize all scope variable types to i64 for the merge block to avoid type mismatches.
        let i64t = self.i64_type();
        let merge_arg_types: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            scope_keys.iter().map(|_| (i64t, self.loc)).collect();
        // Convert all current scope values to i64 before branching.
        let init_i64_vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| {
            self.ensure_i64(scope[k], block).unwrap_or(scope[k])
        }).collect();
        // Update scope to use i64 versions.
        for (k, v) in scope_keys.iter().zip(init_i64_vals.iter()) {
            scope.insert(k.clone(), *v);
        }

        let then_block  = region.append_block(Block::new(&[]));
        let else_block  = region.append_block(Block::new(&[]));
        let merge_block = region.append_block(Block::new(&merge_arg_types));

        block.append_operation(cf::cond_br(
            self.ctx, cond_i1, &then_block, &else_block, &[], &[], self.loc,
        ));

        // Then branch
        let mut then_scope = scope.clone();
        let (_, then_end) = self.lower_statement(&if_stmt.consequent, then_block, region, &mut then_scope, loops)?;
        // Collect values as i64; only if block is not yet terminated.
        let then_vals: Vec<Value<'c, 'b>> = if then_end.terminator().is_none() {
            scope_keys.iter().map(|k| {
                let v = *then_scope.get(k).unwrap_or(&scope[k]);
                self.ensure_i64(v, then_end).unwrap_or(v)
            }).collect()
        } else { scope_keys.iter().map(|k| scope[k]).collect() };
        self.terminate_with_br(then_end, &merge_block, &then_vals);

        // Else branch
        let mut else_scope = scope.clone();
        if let Some(alt) = &if_stmt.alternate {
            let (_, else_end) = self.lower_statement(alt, else_block, region, &mut else_scope, loops)?;
            let else_vals: Vec<Value<'c, 'b>> = if else_end.terminator().is_none() {
                scope_keys.iter().map(|k| {
                    let v = *else_scope.get(k).unwrap_or(&scope[k]);
                    self.ensure_i64(v, else_end).unwrap_or(v)
                }).collect()
            } else { scope_keys.iter().map(|k| scope[k]).collect() };
            self.terminate_with_br(else_end, &merge_block, &else_vals);
        } else {
            let orig_vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
            self.terminate_with_br(else_block, &merge_block, &orig_vals);
        }

        // Update scope to use merge-block phi arguments.
        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), merge_block.argument(i)?.into());
        }

        Ok((None, merge_block))
    }

    // ── While loop  (phi-node header) ────────────────────────────────────

    pub(super) fn lower_while_statement<'b>(
        &mut self,
        while_stmt: &WhileStatement<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
        loops: &[(BlockRef<'c, 'b>, BlockRef<'c, 'b>, Vec<String>)],
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let scope_keys: Vec<String> = scope.keys().cloned().collect();
        // Normalize all scope values to i64 to avoid type mismatches in phi nodes.
        let i64t = self.i64_type();
        for k in &scope_keys {
            let v64 = self.ensure_i64(scope[k], block)?;
            scope.insert(k.clone(), v64);
        }
        let phi_types: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            scope_keys.iter().map(|_| (i64t, self.loc)).collect();

        // header receives all scope vars as block arguments (loop-carried values).
        let mut header_block = region.append_block(Block::new(&phi_types));
        let body_block   = region.append_block(Block::new(&[]));
        let exit_block   = region.append_block(Block::new(&phi_types));

        // Jump into the header with initial values.
        let init_vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
        block.append_operation(cf::br(&header_block, &init_vals, self.loc));

        // Build scope for the header (use block arguments).
        let mut header_scope = scope.clone();
        for (i, k) in scope_keys.iter().enumerate() {
            header_scope.insert(k.clone(), header_block.argument(i)?.into());
        }

        // Evaluate condition inside the header.
        let (cond_opt, nb) = self.lower_expression(&while_stmt.test, header_block, region, &mut header_scope)?;
        header_block = nb;
        let cond_val = cond_opt.ok_or_else(|| anyhow::anyhow!("while condition must produce a value"))?;
        let cond_i1 = self.ensure_i1(cond_val, header_block)?;

        // The exit block gets the header-block values when the condition is false.
        let header_vals: Vec<Value<'c, 'b>> = (0..scope_keys.len())
            .map(|i| header_block.argument(i).map(Into::into))
            .collect::<Result<_, _>>()?;
        header_block.append_operation(cf::cond_br(
            self.ctx, cond_i1, &body_block, &exit_block, &[], &header_vals, self.loc,
        ));

        // Lower the loop body.
        let mut body_scope = header_scope.clone();
        let mut inner_loops = loops.to_vec();
        inner_loops.push((header_block, exit_block, scope_keys.clone()));
        let (_, body_end) =
            self.lower_statement(&while_stmt.body, body_block, region, &mut body_scope, &inner_loops)?;
        let body_vals: Vec<Value<'c, 'b>> = if body_end.terminator().is_none() {
            scope_keys.iter().map(|k| {
                let v = *body_scope.get(k).unwrap_or(&header_scope[k]);
                self.ensure_i64(v, body_end).unwrap_or(v)
            }).collect()
        } else { scope_keys.iter().map(|k| header_scope[k]).collect() };
        self.terminate_with_br(body_end, &header_block, &body_vals);

        // After the loop, scope uses exit-block arguments.
        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), exit_block.argument(i)?.into());
        }

        Ok((None, exit_block))
    }

    // ── For loop (desugared: init + while) ───────────────────────────────

    pub(super) fn lower_for_statement<'b>(
        &mut self,
        for_stmt: &ForStatement<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
        loops: &[(BlockRef<'c, 'b>, BlockRef<'c, 'b>, Vec<String>)],
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        // Lower init (may introduce new variables into scope).
        let mut current = block;
        if let Some(init) = &for_stmt.init {
            match init {
                ForStatementInit::VariableDeclaration(vd) => {
                    self.lower_variable_declaration(vd, current, region, scope)?;
                }
                _ => {
                    // Treat as an expression (ForStatementInit inherits Expression variants).
                    let expr = init.as_expression().ok_or_else(|| {
                        anyhow::anyhow!("unsupported for-loop init")
                    })?;
                    let (_, nb) = self.lower_expression(expr, current, region, scope)?;
                    current = nb;
                }
            }
        }

        let scope_keys: Vec<String> = scope.keys().cloned().collect();
        // Normalize all scope values to i64 for consistent phi types.
        let i64t = self.i64_type();
        for k in &scope_keys {
            let v64 = self.ensure_i64(scope[k], current)?;
            scope.insert(k.clone(), v64);
        }
        let phi_types: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            scope_keys.iter().map(|_| (i64t, self.loc)).collect();

        let mut header_block = region.append_block(Block::new(&phi_types));
        let body_block   = region.append_block(Block::new(&[]));
        let exit_block   = region.append_block(Block::new(&phi_types));

        let init_vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
        current.append_operation(cf::br(&header_block, &init_vals, self.loc));

        // Header scope: use block arguments.
        let mut header_scope = scope.clone();
        for (i, k) in scope_keys.iter().enumerate() {
            header_scope.insert(k.clone(), header_block.argument(i)?.into());
        }

        // Evaluate condition (or default to `true` if absent).
        let cond_i1 = if let Some(test) = &for_stmt.test {
            let (cv_opt, nb) = self.lower_expression(test, header_block, region, &mut header_scope)?;
            header_block = nb;
            let cv = cv_opt.ok_or_else(|| anyhow::anyhow!("for condition must produce a value"))?;
            self.ensure_i1(cv, header_block)?
        } else {
            self.lower_boolean_literal(true, header_block)?
        };

        let header_vals: Vec<Value<'c, 'b>> = (0..scope_keys.len())
            .map(|i| header_block.argument(i).map(Into::into))
            .collect::<Result<_, _>>()?;
            
        // We evaluate condition, if true jump to body_block, else exit_block.
        header_block.append_operation(cf::cond_br(
            self.ctx, cond_i1, &body_block, &exit_block, &[], &header_vals, self.loc,
        ));

        // Create an update block for `continue` statements to securely jump to.
        let mut update_block = region.append_block(Block::new(&phi_types));

        // Lower body.
        let mut body_scope = header_scope.clone();
        let mut inner_loops = loops.to_vec();
        // continue jumps to the update_block, while break jumps to the exit_block.
        inner_loops.push((update_block, exit_block, scope_keys.clone()));
        
        let (_, body_end) =
            self.lower_statement(&for_stmt.body, body_block, region, &mut body_scope, &inner_loops)?;

        // Normal end of body also jumps to the update block.
        let body_vals: Vec<Value<'c, 'b>> = if body_end.terminator().is_none() {
            scope_keys.iter().zip(phi_types.iter()).map(|(k, (ty, _))| {
                let v = *body_scope.get(k).unwrap_or(&header_scope[k]);
                self.coerce_val_to_type(v, *ty, body_end).unwrap_or(v)
            }).collect()
        } else { scope_keys.iter().map(|k| header_scope[k]).collect() };
        self.terminate_with_br(body_end, &update_block, &body_vals);

        // Lower update expression inside the update block.
        let mut update_scope = header_scope.clone();
        for (i, k) in scope_keys.iter().enumerate() {
            update_scope.insert(k.clone(), update_block.argument(i)?.into());
        }
        
        if let Some(update) = &for_stmt.update {
            let (_, nb) = self.lower_expression(update, update_block, region, &mut update_scope)?;
            update_block = nb;
        }

        // Finally, the update block jumps unconditionally back to the header block.
        let update_vals: Vec<Value<'c, 'b>> = if update_block.terminator().is_none() {
            scope_keys.iter().zip(phi_types.iter()).map(|(k, (ty, _))| {
                let v = *update_scope.get(k).unwrap_or(&header_scope[k]);
                self.coerce_val_to_type(v, *ty, update_block).unwrap_or(v)
            }).collect()
        } else { scope_keys.iter().map(|k| header_scope[k]).collect() };
        self.terminate_with_br(update_block, &header_block, &update_vals);

        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), exit_block.argument(i)?.into());
        }

        Ok((None, exit_block))
    }

    // ── throw statement ───────────────────────────────────────────────────

    pub(super) fn lower_throw_statement<'b>(
        &mut self,
        throw: &oxc_ast::ast::ThrowStatement<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let (val_opt, nb) = self.lower_expression(&throw.argument, block, region, scope)?;
        block = nb;
        let val = val_opt.ok_or_else(|| anyhow::anyhow!("throw: expression produced no value"))?;
        let val_i64 = self.ensure_i64(val, block)?;

        block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_throw"),
            &[val_i64],
            &[],
            self.loc,
        ));

        // Return the SAME block so the enclosing try-body loop can emit
        // an exception check here.  Any code after the throw is unreachable
        // in practice (the check always branches away), but semantically fine.
        Ok((None, block))
    }

    // ── try / catch / finally ─────────────────────────────────────────────

    pub(super) fn lower_try_statement<'b>(
        &mut self,
        try_stmt: &oxc_ast::ast::TryStatement<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
        loops: &[(BlockRef<'c, 'b>, BlockRef<'c, 'b>, Vec<String>)],
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let i32_type = self.i32_type();
        let i64_type = self.i64_type();

        // Snapshot the outer scope before entering the try body.
        // Normalize all scope values to i64 for consistent phi types.
        let scope_keys: Vec<String> = scope.keys().cloned().collect();
        for k in &scope_keys {
            let v64 = self.ensure_i64(scope[k], block)?;
            scope.insert(k.clone(), v64);
        }
        let phi_types: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            scope_keys.iter().map(|_| (i64_type, self.loc)).collect();

        let has_catch   = try_stmt.handler.is_some();
        let has_finally = try_stmt.finalizer.is_some();

        // Allocate the destination blocks that different paths converge into.
        let catch_block   = has_catch.then(|| region.append_block(Block::new(&phi_types)));
        let finally_block = has_finally.then(|| region.append_block(Block::new(&phi_types)));
        let merge_block   = region.append_block(Block::new(&phi_types));

        // Helper: find the "error" target (catch → finally → merge).
        let error_block: &Block<'c> = catch_block.as_ref()
            .map(|b| &**b)
            .or(finally_block.as_ref().map(|b| -> &Block<'c> { &**b }))
            .unwrap_or(&*merge_block);

        // Helper: where normal-path flow goes after the try body.
        let normal_block: &Block<'c> = finally_block.as_ref()
            .map(|b| -> &Block<'c> { &**b })
            .unwrap_or(&*merge_block);

        // ── Try body ─────────────────────────────────────────────────────
        let mut try_scope = scope.clone();
        let mut cur = block;

        for stmt in &try_stmt.block.body {
            if matches!(stmt, Statement::FunctionDeclaration(_)) { continue; }

            let (_, nb) = self.lower_statement(stmt, cur, region, &mut try_scope, loops)?;
            cur = nb;

            // Check exception flag after each statement.
            let exc_i32: Value<'c, 'b> = cur.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_check_exception"),
                &[],
                &[i32_type],
                self.loc,
            )).result(0)?.into();

            let zero: Value<'c, 'b> = cur.append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(i32_type, 0).into(),
                self.loc,
            )).result(0)?.into();

            let is_exc: Value<'c, 'b> = cur.append_operation(arith::cmpi(
                self.ctx, arith::CmpiPredicate::Ne, exc_i32, zero, self.loc,
            )).result(0)?.into();

            // Current values of pre-try scope vars to pass to the error target.
            // Coerce to expected phi types to handle e.g. i64 assigned to an i32 var.
            let exc_vals: Vec<Value<'c, 'b>> = scope_keys.iter().enumerate()
                .map(|(i, k)| {
                    let v = *try_scope.get(k).unwrap_or_else(|| &scope[k]);
                    self.coerce_val_to_type(v, phi_types[i].0, cur)
                })
                .collect::<Result<_>>()?;

            // Continuation block for the no-exception path (no args needed).
            let cont = region.append_block(Block::new(&[]));
            cur.append_operation(cf::cond_br(
                self.ctx, is_exc, error_block, &cont, &exc_vals, &[], self.loc,
            ));
            cur = cont;
        }

        // Try completed without exception → jump to finally/merge.
        let ok_vals: Vec<Value<'c, 'b>> = scope_keys.iter().enumerate()
            .map(|(i, k)| {
                let v = *try_scope.get(k).unwrap_or_else(|| &scope[k]);
                self.coerce_val_to_type(v, phi_types[i].0, cur)
            })
            .collect::<Result<_>>()?;
        self.terminate_with_br(cur, normal_block, &ok_vals);

        // ── Catch block ───────────────────────────────────────────────────
        if let (Some(ref cb), Some(handler)) = (&catch_block, &try_stmt.handler) {
            let mut catch_scope = scope.clone();
            // Rebuild scope from the phi block-arguments.
            for (i, k) in scope_keys.iter().enumerate() {
                catch_scope.insert(k.clone(), cb.argument(i)?.into());
            }

            // Retrieve the thrown value (clears the exception flag).
            let exc_val: Value<'c, 'b> = cb.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_catch_exception"),
                &[],
                &[i64_type],
                self.loc,
            )).result(0)?.into();

            // Bind the catch parameter (e.g. `catch (e)`).
            if let Some(param) = &handler.param {
                let param_name = match &param.pattern {
                    BindingPattern::BindingIdentifier(id) => id.name.to_string(),
                    _ => "_err".to_string(),
                };
                catch_scope.insert(param_name, exc_val);
            }

            let mut catch_cur: BlockRef<'c, 'b> = *cb;
            for stmt in &handler.body.body {
                let (_, nb) = self.lower_statement(stmt, catch_cur, region, &mut catch_scope, loops)?;
                catch_cur = nb;
            }

            // Release catch-local variables before leaving the block.
            for (k, v) in &catch_scope {
                if !scope_keys.contains(k) {
                    let v_i64 = self.ensure_i64(*v, catch_cur)?;
                    catch_cur.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[v_i64], &[], self.loc,
                    ));
                }
            }

            let catch_vals: Vec<Value<'c, 'b>> = scope_keys.iter().enumerate()
                .map(|(i, k)| {
                    let v = *catch_scope.get(k).unwrap_or_else(|| &scope[k]);
                    self.coerce_val_to_type(v, phi_types[i].0, catch_cur)
                })
                .collect::<Result<_>>()?;
            self.terminate_with_br(catch_cur, normal_block, &catch_vals);
        }

        // ── Finally block ─────────────────────────────────────────────────
        if let (Some(ref fb), Some(finalizer)) = (&finally_block, &try_stmt.finalizer) {
            let mut fin_scope = scope.clone();
            for (i, k) in scope_keys.iter().enumerate() {
                fin_scope.insert(k.clone(), fb.argument(i)?.into());
            }

            let mut fin_cur: BlockRef<'c, 'b> = *fb;
            for stmt in &finalizer.body {
                let (_, nb) = self.lower_statement(stmt, fin_cur, region, &mut fin_scope, loops)?;
                fin_cur = nb;
            }

            let fin_vals: Vec<Value<'c, 'b>> = scope_keys.iter().enumerate()
                .map(|(i, k)| {
                    let v = *fin_scope.get(k).unwrap_or_else(|| &scope[k]);
                    self.coerce_val_to_type(v, phi_types[i].0, fin_cur)
                })
                .collect::<Result<_>>()?;
            self.terminate_with_br(fin_cur, &merge_block, &fin_vals);
        }

        // ── Update outer scope from merge block ───────────────────────────
        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), merge_block.argument(i)?.into());
        }

        Ok((None, merge_block))
    }

    // ── for...of ──────────────────────────────────────────────────────────

    pub(super) fn lower_for_of_statement<'b>(
        &mut self,
        for_of: &ForOfStatement<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
        loops: &[(BlockRef<'c, 'b>, BlockRef<'c, 'b>, Vec<String>)],
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        // Determine the loop variable binding.
        // `loop_var` is the name used to hold the raw element in body_scope (always internal or user).
        // `destructure_vars` holds (index, name) pairs if the LHS is an ArrayPattern.
        let (loop_var, destructure_vars) = match &for_of.left {
            ForStatementLeft::VariableDeclaration(vd) => {
                if let Some(d) = vd.declarations.first() {
                    match &d.id {
                        BindingPattern::BindingIdentifier(id) => {
                            (id.name.to_string(), vec![])
                        }
                        BindingPattern::ArrayPattern(arr_pat) => {
                            // Destructure: use internal name for the element, extract sub-vars below.
                            let sub_vars: Vec<(usize, String)> = arr_pat.elements.iter().enumerate()
                                .filter_map(|(i, elem)| {
                                    if let Some(BindingPattern::BindingIdentifier(id)) = elem {
                                        Some((i, id.name.to_string()))
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            ("__forof_item".to_string(), sub_vars)
                        }
                        _ => ("__forof_item".to_string(), vec![])
                    }
                } else {
                    ("__forof_item".to_string(), vec![])
                }
            }
            _ => ("__forof_item".to_string(), vec![]),
        };

        // Evaluate the iterable (must be an array).
        let (iter_opt, nb) = self.lower_expression(&for_of.right, block, region, scope)?;
        block = nb;
        let iter_val = iter_opt.ok_or_else(|| anyhow::anyhow!("for...of: iterable produced no value"))?;
        let iter_i64 = self.ensure_i64(iter_val, block)?;

        // Get array length (returns TsVal; unbox to i32).
        let len_tsval: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_len"),
            &[iter_i64],
            &[self.i64_type()],
            self.loc,
        )).result(0)?.into();
        let len_i32 = self.ensure_i32(len_tsval, block)?;

        // Index starts at 0.
        let zero_i32 = self.lower_numeric_literal(0, block)?;

        // Normalize len/idx to i64 for consistent phi types.
        let len_i64 = self.ensure_i64(len_i32, block)?;
        let zero_i64 = self.ensure_i64(zero_i32, block)?;

        // Store iter + len + idx as loop-carried scope entries with unique names.
        let iter_key = "__forofiter__".to_string();
        let len_key  = "__foroflen__".to_string();
        let idx_key  = "__forofidx__".to_string();
        scope.insert(iter_key.clone(), iter_i64);
        scope.insert(len_key.clone(),  len_i64);
        scope.insert(idx_key.clone(),  zero_i64);

        // Normalize all outer scope values to i64 before creating phi nodes.
        let i64t = self.i64_type();
        let scope_keys: Vec<String> = scope.keys().cloned().collect();
        for k in &scope_keys {
            let v64 = self.ensure_i64(scope[k], block)?;
            scope.insert(k.clone(), v64);
        }
        let phi_types: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            scope_keys.iter().map(|_| (i64t, self.loc)).collect();

        let mut header_block = region.append_block(Block::new(&phi_types));
        let body_block       = region.append_block(Block::new(&[]));
        let update_block     = region.append_block(Block::new(&phi_types));
        let exit_block       = region.append_block(Block::new(&phi_types));

        // Jump into header with initial values.
        let init_vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
        block.append_operation(cf::br(&header_block, &init_vals, self.loc));

        // ── Header: rebuild scope from phi args, check idx < len ─────────────
        let mut header_scope = scope.clone();
        for (i, k) in scope_keys.iter().enumerate() {
            header_scope.insert(k.clone(), header_block.argument(i)?.into());
        }

        let idx_hdr = self.ensure_i32(header_scope[&idx_key], header_block)?;
        let len_hdr = self.ensure_i32(header_scope[&len_key],  header_block)?;
        let cond_i1: Value<'c, 'b> = header_block.append_operation(arith::cmpi(
            self.ctx, arith::CmpiPredicate::Slt, idx_hdr, len_hdr, self.loc,
        )).result(0)?.into();

        let header_args: Vec<Value<'c, 'b>> = (0..scope_keys.len())
            .map(|i| header_block.argument(i).map(Into::into))
            .collect::<Result<_, _>>()?;
        header_block.append_operation(cf::cond_br(
            self.ctx, cond_i1, &body_block, &exit_block, &[], &header_args, self.loc,
        ));

        // ── Body: fetch element, execute body ────────────────────────────────
        let mut body_scope = header_scope.clone();
        let iter_body  = self.ensure_i64(body_scope[&iter_key], body_block)?;
        let idx_body   = self.ensure_i32(body_scope[&idx_key],  body_block)?;

        let elem: Value<'c, 'b> = body_block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
            &[iter_body, idx_body],
            &[self.i64_type()],
            self.loc,
        )).result(0)?.into();

        body_scope.insert(loop_var.clone(), elem);

        // If the LHS is an ArrayPattern, destructure the element into sub-variables.
        if !destructure_vars.is_empty() {
            for (i, sub_name) in &destructure_vars {
                let sub_idx_val: Value<'c, 'b> = body_block.append_operation(arith::constant(
                    self.ctx,
                    IntegerAttribute::new(self.i32_type(), *i as i64).into(),
                    self.loc,
                )).result(0)?.into();
                let sub_val: Value<'c, 'b> = body_block.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
                    &[elem, sub_idx_val],
                    &[self.i64_type()],
                    self.loc,
                )).result(0)?.into();
                body_scope.insert(sub_name.clone(), sub_val);
            }
        }

        let mut inner_loops = loops.to_vec();
        inner_loops.push((update_block, exit_block, scope_keys.clone()));
        let (_, body_end) = self.lower_statement(&for_of.body, body_block, region, &mut body_scope, &inner_loops)?;

        // Release destructured sub-variables before leaving the body.
        for (_, sub_name) in &destructure_vars {
            if let Some(&sub_val) = body_scope.get(sub_name) {
                let sub_i64 = self.ensure_i64(sub_val, body_end)?;
                body_end.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[sub_i64], &[], self.loc,
                ));
            }
        }

        // Release the loop variable (the element / pair array) before leaving the body.
        if let Some(&loop_val) = body_scope.get(&loop_var) {
            let loop_val_i64 = self.ensure_i64(loop_val, body_end)?;
            body_end.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[loop_val_i64], &[], self.loc,
            ));
        }

        // body_vals: coerce types to match phi_types.
        let body_vals: Vec<Value<'c, 'b>> = if body_end.terminator().is_none() {
            scope_keys.iter().enumerate()
                .map(|(i, k)| {
                    let v = *body_scope.get(k).unwrap_or(&header_scope[k]);
                    self.coerce_val_to_type(v, phi_types[i].0, body_end)
                })
                .collect::<Result<_>>()?
        } else { scope_keys.iter().map(|k| header_scope[k]).collect() };
        self.terminate_with_br(body_end, &update_block, &body_vals);

        // ── Update: increment index ───────────────────────────────────────────
        let mut update_scope = header_scope.clone();
        for (i, k) in scope_keys.iter().enumerate() {
            update_scope.insert(k.clone(), update_block.argument(i)?.into());
        }
        let idx_upd = self.ensure_i32(update_scope[&idx_key], update_block)?;
        let one_i32 = self.lower_numeric_literal(1, update_block)?;
        let new_idx_i32: Value<'c, 'b> = update_block.append_operation(
            arith::addi(idx_upd, one_i32, self.loc)
        ).result(0)?.into();
        let new_idx_i64 = self.ensure_i64(new_idx_i32, update_block)?;
        update_scope.insert(idx_key.clone(), new_idx_i64);

        let update_vals: Vec<Value<'c, 'b>> = if update_block.terminator().is_none() {
            scope_keys.iter().enumerate()
                .map(|(i, k)| {
                    let v = *update_scope.get(k).unwrap_or(&header_scope[k]);
                    self.coerce_val_to_type(v, phi_types[i].0, update_block)
                })
                .collect::<Result<_>>()?
        } else { scope_keys.iter().map(|k| header_scope[k]).collect() };
        self.terminate_with_br(update_block, &header_block, &update_vals);

        // ── Exit: update outer scope, release iter ────────────────────────────
        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), exit_block.argument(i)?.into());
        }
        let iter_exit = self.ensure_i64(scope[&iter_key], exit_block)?;
        exit_block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[iter_exit], &[], self.loc,
        ));

        // Remove internal loop-carry vars from outer scope.
        scope.remove(&iter_key);
        scope.remove(&len_key);
        scope.remove(&idx_key);

        Ok((None, exit_block))
    }

    // ── for...in ──────────────────────────────────────────────────────────

    pub(super) fn lower_for_in_statement<'b>(
        &mut self,
        for_in: &ForInStatement<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
        loops: &[(BlockRef<'c, 'b>, BlockRef<'c, 'b>, Vec<String>)],
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        // Evaluate the object.
        let (obj_opt, nb) = self.lower_expression(&for_in.right, block, region, scope)?;
        block = nb;
        let obj_val = obj_opt.ok_or_else(|| anyhow::anyhow!("for...in: object produced no value"))?;
        let obj_i64 = self.ensure_i64(obj_val, block)?;

        // Get the array of keys via ts_obj_keys(obj).
        let keys_arr: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_obj_keys"),
            &[obj_i64],
            &[self.i64_type()],
            self.loc,
        )).result(0)?.into();

        // Release obj (keys_arr owns the strings now).
        block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[obj_i64], &[], self.loc,
        ));

        // Reuse the for...of infrastructure by building a synthetic ForOfStatement equivalent.
        // Determine loop variable name.
        let loop_var = match &for_in.left {
            ForStatementLeft::VariableDeclaration(vd) => {
                vd.declarations.first().and_then(|d| {
                    if let BindingPattern::BindingIdentifier(id) = &d.id {
                        Some(id.name.to_string())
                    } else { None }
                })
            }
            _ => None,
        }.unwrap_or_else(|| "__forin_item__".to_string());

        // Store keys_arr in scope under a temp name, then iterate over it.
        let keys_key = "__forinarr__".to_string();
        scope.insert(keys_key.clone(), keys_arr);

        // Get length of keys array.
        let len_tsval: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_len"),
            &[scope[&keys_key]],
            &[self.i64_type()],
            self.loc,
        )).result(0)?.into();
        let len_i32 = self.ensure_i32(len_tsval, block)?;

        let zero_i32 = self.lower_numeric_literal(0, block)?;

        let len_i64 = self.ensure_i64(len_i32, block)?;
        let zero_i64 = self.ensure_i64(zero_i32, block)?;

        let len_key = "__forinlen__".to_string();
        let idx_key = "__forinidx__".to_string();
        scope.insert(len_key.clone(), len_i64);
        scope.insert(idx_key.clone(), zero_i64);

        // Normalize all scope values to i64 before creating phi nodes.
        let i64t = self.i64_type();
        let scope_keys: Vec<String> = scope.keys().cloned().collect();
        for k in &scope_keys {
            let v64 = self.ensure_i64(scope[k], block)?;
            scope.insert(k.clone(), v64);
        }
        let phi_types: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            scope_keys.iter().map(|_| (i64t, self.loc)).collect();

        let mut header_block = region.append_block(Block::new(&phi_types));
        let body_block       = region.append_block(Block::new(&[]));
        let update_block     = region.append_block(Block::new(&phi_types));
        let exit_block       = region.append_block(Block::new(&phi_types));

        let init_vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
        block.append_operation(cf::br(&header_block, &init_vals, self.loc));

        let mut header_scope = scope.clone();
        for (i, k) in scope_keys.iter().enumerate() {
            header_scope.insert(k.clone(), header_block.argument(i)?.into());
        }

        let idx_hdr = self.ensure_i32(header_scope[&idx_key], header_block)?;
        let len_hdr = self.ensure_i32(header_scope[&len_key], header_block)?;
        let cond_i1: Value<'c, 'b> = header_block.append_operation(arith::cmpi(
            self.ctx, arith::CmpiPredicate::Slt, idx_hdr, len_hdr, self.loc,
        )).result(0)?.into();
        let header_args: Vec<Value<'c, 'b>> = (0..scope_keys.len())
            .map(|i| header_block.argument(i).map(Into::into))
            .collect::<Result<_, _>>()?;
        header_block.append_operation(cf::cond_br(
            self.ctx, cond_i1, &body_block, &exit_block, &[], &header_args, self.loc,
        ));

        // Body: get the key string from the keys array.
        let mut body_scope = header_scope.clone();
        let arr_body = self.ensure_i64(body_scope[&keys_key], body_block)?;
        let idx_body = self.ensure_i32(body_scope[&idx_key],  body_block)?;

        let key_val: Value<'c, 'b> = body_block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
            &[arr_body, idx_body],
            &[self.i64_type()],
            self.loc,
        )).result(0)?.into();
        body_scope.insert(loop_var.clone(), key_val);

        let mut inner_loops = loops.to_vec();
        inner_loops.push((update_block, exit_block, scope_keys.clone()));
        let (_, body_end) = self.lower_statement(&for_in.body, body_block, region, &mut body_scope, &inner_loops)?;

        if let Some(&loop_val) = body_scope.get(&loop_var) {
            let loop_val_i64 = self.ensure_i64(loop_val, body_end)?;
            body_end.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[loop_val_i64], &[], self.loc,
            ));
        }

        let body_vals: Vec<Value<'c, 'b>> = if body_end.terminator().is_none() {
            scope_keys.iter().enumerate()
                .map(|(i, k)| {
                    let v = *body_scope.get(k).unwrap_or(&header_scope[k]);
                    self.coerce_val_to_type(v, phi_types[i].0, body_end)
                })
                .collect::<Result<_>>()?
        } else { scope_keys.iter().map(|k| header_scope[k]).collect() };
        self.terminate_with_br(body_end, &update_block, &body_vals);

        let mut update_scope = header_scope.clone();
        for (i, k) in scope_keys.iter().enumerate() {
            update_scope.insert(k.clone(), update_block.argument(i)?.into());
        }
        // Increment index: extract i32, add 1, normalize back to i64.
        let idx_upd_i32 = self.ensure_i32(update_scope[&idx_key], update_block)?;
        let one_i32 = self.lower_numeric_literal(1, update_block)?;
        let new_idx_i32: Value<'c, 'b> = update_block.append_operation(
            arith::addi(idx_upd_i32, one_i32, self.loc)
        ).result(0)?.into();
        let new_idx_i64 = self.ensure_i64(new_idx_i32, update_block)?;
        update_scope.insert(idx_key.clone(), new_idx_i64);

        let update_vals: Vec<Value<'c, 'b>> = if update_block.terminator().is_none() {
            scope_keys.iter().enumerate()
                .map(|(i, k)| {
                    let v = *update_scope.get(k).unwrap_or(&header_scope[k]);
                    self.coerce_val_to_type(v, phi_types[i].0, update_block)
                })
                .collect::<Result<_>>()?
        } else { scope_keys.iter().map(|k| header_scope[k]).collect() };
        self.terminate_with_br(update_block, &header_block, &update_vals);

        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), exit_block.argument(i)?.into());
        }
        // Release the keys array.
        let keys_exit = self.ensure_i64(scope[&keys_key], exit_block)?;
        exit_block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[keys_exit], &[], self.loc,
        ));

        scope.remove(&keys_key);
        scope.remove(&len_key);
        scope.remove(&idx_key);

        Ok((None, exit_block))
    }

}
