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

        // Activate dhat heap profiler if compiled with --features dhat-heap.
        // This is a no-op in normal builds (the function body is empty).
        current_block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_dhat_init"),
            &[],
            &[],
            self.loc,
        ));

        // Call module init functions for imported files (initialize imported const values).
        for init_fn_name in self.module_init_fns.clone() {
            current_block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, &init_fn_name),
                &[],
                &[],
                self.loc,
            ));
        }

        // Compute which top-level variables need cell treatment (mutated inside closures).
        let saved_cell_vars = std::mem::replace(
            &mut self.cell_vars,
            crate::lowering::expressions::compute_cell_vars_for_body(&program.body),
        );
        let saved_cell_captures = std::mem::replace(&mut self.cell_captures, std::collections::HashSet::new());

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

        self.cell_vars = saved_cell_vars;
        self.cell_captures = saved_cell_captures;

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

    // ── Class constructor TsFunction helper ──────────────────────────────

    /// Emit code that creates the constructor TsFunction for a named class in `block`.
    /// Returns `(new_block, ctor_ts_val)` where `ctor_ts_val` is an owned i64 reference.
    pub(super) fn emit_class_constructor_val<'b>(
        &mut self,
        class_name: &str,
        block: BlockRef<'c, 'b>,
    ) -> Result<(BlockRef<'c, 'b>, Value<'c, 'b>)> {
        let ctor_fn_name = format!("__class_{}_constructor", class_name);
        let sig = self.funcs.get(&ctor_fn_name).cloned();
        let n_params = sig.map_or(0, |s| s.param_types.len());
        let i64_type = self.i64_type();
        let i32_type = self.i32_type();
        let ptr_type = melior::dialect::llvm::r#type::pointer(self.ctx, 0);

        let param_types: Vec<melior::ir::Type<'c>> = vec![i64_type; n_params];
        let func_type_val = melior::ir::r#type::FunctionType::new(
            self.ctx,
            &param_types,
            &[i64_type],
        ).into();
        let fn_ref: Value<'c, 'b> = block.append_operation(
            melior::ir::operation::OperationBuilder::new("func.constant", self.loc)
                .add_attributes(&[(
                    melior::ir::Identifier::new(self.ctx, "value"),
                    FlatSymbolRefAttribute::new(self.ctx, &ctor_fn_name).into(),
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
            self.ctx, IntegerAttribute::new(i32_type, n_params as i64).into(), self.loc,
        )).result(0)?.into();
        let ctor_val: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_func_new_this"),
            &[fn_ptr, arity_val], &[i64_type], self.loc,
        )).result(0)?.into();
        Ok((block, ctor_val))
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
        loops: &[(Option<BlockRef<'c, 'b>>, BlockRef<'c, 'b>, Vec<String>, Option<String>)],
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

            Statement::BreakStatement(brk) => {
                // Find target: labeled break searches for matching label; unlabeled breaks innermost.
                let target = if let Some(label) = brk.label.as_ref() {
                    let lname = label.name.as_str();
                    loops.iter().rev().find(|(_, _, _, lbl)| lbl.as_deref() == Some(lname))
                } else {
                    loops.last()
                };
                if let Some((_, exit_block, scope_keys, _)) = target {
                    // Coerce each scope value to the expected phi arg type of exit_block.
                    // Switch exit blocks force i64; loop exit blocks may use other types.
                    let mut vals: Vec<Value<'c, 'b>> = Vec::new();
                    for (i, k) in scope_keys.iter().enumerate() {
                        let v = scope[k];
                        let coerced = if let Ok(arg) = exit_block.argument(i) {
                            self.coerce_val_to_type(v, arg.r#type(), block)?
                        } else { v };
                        vals.push(coerced);
                    }
                    self.terminate_with_br(block, exit_block, &vals);
                    let dead = region.append_block(Block::new(&[]));
                    Ok((None, dead))
                } else {
                    bail!("break statement outside of loop or missing label");
                }
            }
            Statement::ContinueStatement(cont) => {
                // Skip switch entries (None continue target) to find the enclosing loop.
                // For labeled continue, find the entry with that label.
                let loop_entry = if let Some(label) = cont.label.as_ref() {
                    let lname = label.name.as_str();
                    loops.iter().rev()
                        .find(|(cont_opt, _, _, lbl)| cont_opt.is_some() && lbl.as_deref() == Some(lname))
                        .map(|(cont_opt, _, keys, _)| (cont_opt.unwrap(), keys))
                } else {
                    loops.iter().rev()
                        .find_map(|(cont_opt, _, keys, _)| cont_opt.map(|h| (h, keys)))
                };
                if let Some((header_block, scope_keys)) = loop_entry {
                    let vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
                    self.terminate_with_br(block, &header_block, &vals);
                    let dead = region.append_block(Block::new(&[]));
                    Ok((None, dead))
                } else {
                    bail!("continue statement outside of loop");
                }
            }
            Statement::SwitchStatement(sw) => {
                self.lower_switch_statement(sw, block, region, scope, loops)
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
            Statement::ClassDeclaration(class) => {
                // If the class has class-level decorators, create the constructor TsFunction
                // in this scope and apply each decorator in reverse order.
                // Classes without decorators are fully handled by the dedicated hoisting pass.
                if class.decorators.is_empty() {
                    return Ok((None, block));
                }
                let class_name = match &class.id {
                    Some(id) => id.name.to_string(),
                    None => return Ok((None, block)),
                };
                let (block, ctor_val) = self.emit_class_constructor_val(&class_name, block)?;
                // Apply class decorators in reverse order (bottom-to-top).
                let mut cur_val = ctor_val;
                let mut cur_block = block;
                for dec in class.decorators.iter().rev() {
                    let (dec_opt, nb) = self.lower_expression(&dec.expression, cur_block, region, scope)?;
                    cur_block = nb;
                    if let Some(dec_fn) = dec_opt {
                        let dec_fn_i64 = self.ensure_i64(dec_fn, cur_block)?;
                        let cur_i64 = self.ensure_i64(cur_val, cur_block)?;
                        let new_val: Value<'c, '_> = cur_block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_func_call1"),
                            &[dec_fn_i64, cur_i64], &[self.i64_type()], self.loc,
                        )).result(0)?.into();
                        cur_block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                            &[dec_fn_i64], &[], self.loc,
                        ));
                        // If the decorator returns undefined, keep the original class.
                        // Otherwise replace it (standard class decorator semantics).
                        let i64t = self.i64_type();
                        let undef_c: Value<'c, '_> = cur_block.append_operation(arith::constant(
                            self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                        )).result(0)?.into();
                        let is_undef: Value<'c, '_> = cur_block.append_operation(arith::cmpi(
                            self.ctx, arith::CmpiPredicate::Eq, new_val, undef_c, self.loc,
                        )).result(0)?.into();
                        // kept = is_undef ? cur_i64 : new_val
                        let kept: Value<'c, '_> = cur_block.append_operation(
                            melior::dialect::arith::select(is_undef, cur_i64, new_val, self.loc),
                        ).result(0)?.into();
                        // The value we're not keeping needs to be released.
                        // We retain `kept` and release whichever of {cur_i64, new_val} we didn't pick.
                        // Use: release(is_undef ? new_val : cur_i64)
                        let discarded: Value<'c, '_> = cur_block.append_operation(
                            melior::dialect::arith::select(is_undef, new_val, cur_i64, self.loc),
                        ).result(0)?.into();
                        cur_block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"),
                            &[kept], &[], self.loc,
                        ));
                        cur_block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                            &[discarded], &[], self.loc,
                        ));
                        cur_val = kept;
                    }
                }
                // Store the (possibly replaced) class constructor in scope.
                scope.insert(class_name, cur_val);
                Ok((None, cur_block))
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
            Statement::LabeledStatement(labeled) => {
                // Set `pending_label` so the inner loop/switch attaches this label to its
                // loops entry, enabling `break <label>` / `continue <label>` to find it.
                self.pending_label = Some(labeled.label.name.to_string());
                let result = self.lower_statement(&labeled.body, block, region, scope, loops);
                // Clear in case the body wasn't a loop/switch and never consumed the label.
                self.pending_label = None;
                result
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
                        if self.is_cell_var(&name) {
                            let cell = self.alloc_cell(undef, block)?;
                            scope.insert(name.clone(), cell);
                        } else {
                            scope.insert(name.clone(), undef);
                        }
                    }
                    if let Some(init) = &declarator.init {
                        // Class expression: `const Foo = class<T> extends Bar<T> { ... }`
                        // Lower as a class declaration with the variable name as the class name.
                        if let Expression::ClassExpression(class_expr) = init {
                            self.lower_class_declaration_with_name(&name, class_expr)?;
                            // Store undefined as placeholder (class is accessed statically via new Foo())
                            let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                                self.ctx,
                                IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(),
                                self.loc,
                            )).result(0)?.into();
                            scope.insert(name.clone(), undef);
                            continue;
                        }
                        // Type inference: record class name for `let x = new Foo()`.
                        if let Expression::NewExpression(new_expr) = init {
                            if let Expression::Identifier(id) = &new_expr.callee {
                                self.var_class_types.insert(name.clone(), id.name.to_string());
                            }
                        }
                        let (val_opt, nb) = self.lower_expression(init, block, region, scope)?;
                        block = nb;
                        if let Some(val) = val_opt {
                            if self.is_cell_var(&name) {
                                let cell = self.alloc_cell(val, block)?;
                                scope.insert(name.clone(), cell);
                            } else {
                                scope.insert(name.clone(), val);
                            }
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
                        // Handle nested object destructuring: { x: { y } }
                        if let BindingPattern::ObjectPattern(nested_obj) = &prop.value {
                            let key_ptr = self.get_string_ptr(&key_str, block)?;
                            let nested_val: Value<'c, 'b> = block.append_operation(func::call(
                                self.ctx,
                                FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                                &[obj_i64, key_ptr],
                                &[self.i64_type()],
                                self.loc,
                            )).result(0)?.into();
                            extracted_keys.push(key_str);
                            for nested_prop in &nested_obj.properties {
                                let nested_key = match nested_prop.key.static_name() {
                                    Some(n) => n.into_owned(),
                                    None => continue,
                                };
                                let nkey_ptr = self.get_string_ptr(&nested_key, block)?;
                                let nfield: Value<'c, 'b> = block.append_operation(func::call(
                                    self.ctx,
                                    FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                                    &[nested_val, nkey_ptr],
                                    &[self.i64_type()],
                                    self.loc,
                                )).result(0)?.into();
                                let nvar = match &nested_prop.value {
                                    BindingPattern::BindingIdentifier(id) => id.name.to_string(),
                                    BindingPattern::AssignmentPattern(ap) => {
                                        if let BindingPattern::BindingIdentifier(id) = &ap.left {
                                            id.name.to_string()
                                        } else { continue }
                                    }
                                    _ => continue,
                                };
                                scope.insert(nvar, nfield);
                            }
                            continue;
                        }

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
        // ARC: ts_promise_resolve retains val internally, so we must release our
        // owned reference to val after the call (ownership transferred to promise).
        let val = if self.is_async {
            let val_i64 = self.ensure_i64(val, block)?;
            let promise: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_promise_resolve"),
                &[val_i64],
                &[self.i64_type()],
                self.loc,
            )).result(0)?.into();
            // Release the original owned reference — promise now owns it.
            block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[val_i64],
                &[],
                self.loc,
            ));
            promise
        } else {
            val
        };

        // ARC: Release local variables in the current scope before returning.
        // Skip "__env" (env array is borrowed from the closure caller) and function
        // parameters (they are borrowed refs; the call site's post-call ts_release_val
        // balances the pre-call ts_retain_val done by lower_expression).
        for (name, v) in scope.iter() {
            if name == "__env" { continue; }
            if self.current_fn_params.contains(name) { continue; }
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
        loops: &[(Option<BlockRef<'c, 'b>>, BlockRef<'c, 'b>, Vec<String>, Option<String>)],
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
        loops: &[(Option<BlockRef<'c, 'b>>, BlockRef<'c, 'b>, Vec<String>, Option<String>)],
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
        inner_loops.push((Some(header_block), exit_block, scope_keys.clone(), self.pending_label.take()));
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

    // ── Switch statement ──────────────────────────────────────────────────
    //
    // Compiled as a linear chain of equality checks followed by case bodies
    // that fall through to the next case or jump to the exit block on break.
    //
    // Control flow for `switch (disc) { case A: sA; break; case B: sB; default: sD; }`:
    //
    //   entry → check_A → [match] → body_A → [break] → exit
    //                   → check_B → [match] → body_B → (fall) → body_default → exit
    //                             → body_default → exit
    //
    pub(super) fn lower_switch_statement<'b>(
        &mut self,
        sw: &oxc_ast::ast::SwitchStatement<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
        loops: &[(Option<BlockRef<'c, 'b>>, BlockRef<'c, 'b>, Vec<String>, Option<String>)],
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let i64t = self.i64_type();

        // Evaluate discriminant once.
        let (disc_opt, nb) = self.lower_expression(&sw.discriminant, block, region, scope)?;
        block = nb;
        let disc = match disc_opt {
            Some(v) => self.ensure_i64(v, block)?,
            None => bail!("switch discriminant produced no value"),
        };

        // Normalize scope to i64 for phi nodes.
        let scope_keys: Vec<String> = scope.keys().cloned().collect();
        for k in &scope_keys {
            let v64 = self.ensure_i64(scope[k], block)?;
            scope.insert(k.clone(), v64);
        }
        let phi_types: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            scope_keys.iter().map(|_| (i64t, self.loc)).collect();
        let exit_block = region.append_block(Block::new(&phi_types));

        // Separate `default` case from labeled cases.
        let default_case = sw.cases.iter().find(|c| c.test.is_none());
        let labeled_cases: Vec<_> = sw.cases.iter().filter(|c| c.test.is_some()).collect();

        // Create one "check" block (entry of each labeled case's comparison) — no phi args.
        // One "body" block (phi args = all scope vars) per case.
        // Body blocks take phi args so that fallthrough from the previous case can pass
        // updated scope values via SSA, avoiding dominance violations.
        // default_body also takes phi args when it exists.
        let mut check_blocks: Vec<BlockRef<'c, 'b>> = Vec::new();
        let mut body_blocks: Vec<BlockRef<'c, 'b>> = Vec::new();
        for _ in &labeled_cases {
            check_blocks.push(region.append_block(Block::new(&[])));
            body_blocks.push(region.append_block(Block::new(&phi_types)));
        }
        let default_body = if default_case.is_some() {
            region.append_block(Block::new(&phi_types))
        } else {
            // If there's no default, failing all checks goes to exit with current values.
            exit_block
        };

        // Jump from the pre-switch block into the first check (or default_body if no cases).
        if check_blocks.is_empty() {
            let vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
            block.append_operation(cf::br(&default_body, &vals, self.loc));
        } else {
            block.append_operation(cf::br(&check_blocks[0], &[], self.loc));
        }

        // Push a switch entry so `break` inside any case jumps to exit_block.
        // Use `None` for continue so that `continue` inside a switch propagates to
        // the enclosing loop (handled by ContinueStatement searching backwards).
        let mut inner_loops = loops.to_vec();
        inner_loops.push((None, exit_block, scope_keys.clone(), self.pending_label.take()));

        // Build each labeled case's check + body.
        for (idx, case) in labeled_cases.iter().enumerate() {
            let check = check_blocks[idx];
            let body  = body_blocks[idx];
            let next  = if idx + 1 < check_blocks.len() {
                check_blocks[idx + 1]
            } else {
                default_body
            };

            // ── Check block: compare discriminant with case value ──
            // Retain disc for the comparison (it will outlive the check block).
            check.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"),
                &[disc], &[], self.loc,
            ));
            let test_expr = case.test.as_ref().unwrap();
            let mut check_scope = scope.clone();
            let (test_opt, check_end) = self.lower_expression(test_expr, check, region, &mut check_scope)?;
            let test_val = test_opt.ok_or_else(|| anyhow::anyhow!("switch case test produced no value"))?;
            let test_i64 = self.ensure_i64(test_val, check_end)?;
            let eq_i32: Value<'c, 'b> = check_end.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_strict_eq"),
                &[disc, test_i64], &[self.i32_type()], self.loc,
            )).result(0)?.into();
            check_end.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[disc], &[], self.loc,
            ));
            check_end.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[test_i64], &[], self.loc,
            ));
            let c0 = check_end.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(self.i32_type(), 0).into(), self.loc,
            )).result(0)?.into();
            let eq_i1: Value<'c, 'b> = check_end.append_operation(arith::cmpi(
                self.ctx, arith::CmpiPredicate::Ne, eq_i32, c0, self.loc,
            )).result(0)?.into();
            // True branch: jump to body with current scope vals (body block has phi args).
            // False branch: jump to next check (no phi args) or default_body/exit_block (phi args).
            let body_args: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
            let next_args: Vec<Value<'c, 'b>> = if next.argument_count() > 0 {
                scope_keys.iter().map(|k| scope[k]).collect()
            } else {
                vec![]
            };
            check_end.append_operation(cf::cond_br(
                self.ctx, eq_i1, &body, &next, &body_args, &next_args, self.loc,
            ));

            // ── Body block: case consequent ──
            // Initialize case_scope from the body block's phi args so that both the
            // "direct match" path (from check block) and the "fallthrough" path (from
            // the previous case body) use the same properly merged scope values.
            let mut case_scope = scope.clone();
            for (i, k) in scope_keys.iter().enumerate() {
                case_scope.insert(k.clone(), body.argument(i)?.into());
            }
            let mut cur_body = body;
            // `exited` is true when break/return/throw caused a non-local jump, meaning
            // subsequent statements (including our fallthrough) would be dead code.
            let mut exited = false;
            for stmt in &case.consequent {
                let prev = cur_body;
                let (_, nb) = self.lower_statement(stmt, cur_body, region, &mut case_scope, &inner_loops)?;
                cur_body = nb;
                // Detect unconditional exit (break/return/throw): the previous block got a
                // non-branching (unconditional) terminator, meaning control never falls through.
                // `cf.br` is unconditional; `cf.cond_br` (from if/else) is not.
                if let Some(term) = prev.terminator() {
                    let ident = term.name();
                    let sref = ident.as_string_ref();
                    let is_unconditional = sref.as_str() == Ok("cf.br") || sref.as_str() == Ok("func.return");
                    if is_unconditional {
                        exited = true;
                        break;
                    }
                }
                if cur_body.terminator().is_some() {
                    exited = true;
                    break;
                }
            }
            // Emit fallthrough only if we're not in dead code.
            if !exited && cur_body.terminator().is_none() {
                let fall_target = if idx + 1 < body_blocks.len() {
                    body_blocks[idx + 1]
                } else {
                    default_body
                };
                // All fallthrough targets (body blocks and exit_block) take phi args.
                // Coerce case_scope values to i64 and pass them.
                let mut fall_vals: Vec<Value<'c, 'b>> = Vec::new();
                for k in &scope_keys {
                    let v = *case_scope.get(k).unwrap_or(&scope[k]);
                    fall_vals.push(self.coerce_val_to_type(v, i64t, cur_body)?);
                }
                cur_body.append_operation(cf::br(&fall_target, &fall_vals, self.loc));
            }
            // Dead blocks (created by break/return inside the case) have no terminator.
            // Use llvm.unreachable so we don't add fake predecessors to exit_block,
            // which would create phantom CFG paths and break SSA dominance.
            if cur_body.terminator().is_none() {
                cur_body.append_operation(melior::dialect::llvm::unreachable(self.loc));
            }
        }

        // ── Default case body ──
        if let Some(def_case) = default_case {
            // Initialize def_scope from the default_body's phi args (same as for labeled cases).
            let mut def_scope = scope.clone();
            for (i, k) in scope_keys.iter().enumerate() {
                def_scope.insert(k.clone(), default_body.argument(i)?.into());
            }
            let mut cur = default_body;
            let mut exited_default = false;
            for stmt in &def_case.consequent {
                let prev = cur;
                let (_, nb) = self.lower_statement(stmt, cur, region, &mut def_scope, &inner_loops)?;
                cur = nb;
                if let Some(term) = prev.terminator() {
                    let ident = term.name();
                    let sref = ident.as_string_ref();
                    let is_unconditional = sref.as_str() == Ok("cf.br") || sref.as_str() == Ok("func.return");
                    if is_unconditional {
                        exited_default = true;
                        break;
                    }
                }
                if cur.terminator().is_some() {
                    exited_default = true;
                    break;
                }
            }
            if !exited_default && cur.terminator().is_none() {
                let mut vals: Vec<Value<'c, 'b>> = Vec::new();
                for k in &scope_keys {
                    let v = *def_scope.get(k).unwrap_or(&scope[k]);
                    vals.push(self.coerce_val_to_type(v, i64t, cur)?);
                }
                cur.append_operation(cf::br(&exit_block, &vals, self.loc));
            }
            // Same dead-block fix for default case.
            if cur.terminator().is_none() {
                cur.append_operation(melior::dialect::llvm::unreachable(self.loc));
            }
        } else if check_blocks.is_empty() {
            // No cases at all: the initial br to default_body == exit_block already emitted above.
            // But exit_block needs phi args — fix: emit them with a br from block (already done above).
        }

        // If there are no labeled cases and no default, exit_block was branched to directly.
        // Otherwise update scope from exit_block phi args.
        if !phi_types.is_empty() {
            for (i, k) in scope_keys.iter().enumerate() {
                scope.insert(k.clone(), exit_block.argument(i)?.into());
            }
        }

        // Release discriminant after all cases are done.
        // (Each check block retained and released disc itself; here we release the
        // original caller-retained reference.)
        exit_block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[disc], &[], self.loc,
        ));

        Ok((None, exit_block))
    }

    // ── For loop (desugared: init + while) ───────────────────────────────

    pub(super) fn lower_for_statement<'b>(
        &mut self,
        for_stmt: &ForStatement<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
        loops: &[(Option<BlockRef<'c, 'b>>, BlockRef<'c, 'b>, Vec<String>, Option<String>)],
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        // Lower init (may introduce new variables into scope).
        let mut current = block;
        // Save scope before init so we can restore shadowed outer variables after the loop.
        // e.g. `for (let i = 0, len = n; ...)` shadows outer `i` and `len`.
        let pre_init_scope = scope.clone();
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
        inner_loops.push((Some(update_block), exit_block, scope_keys.clone(), self.pending_label.take()));

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

        // Restore outer-scope variables that were shadowed by the for-init's variable
        // declarations (e.g. `for (let i = 0, len = n; ...)` shadows outer `i`/`len`).
        // After the loop, block-scoped init vars are no longer visible; outer bindings resume.
        if let Some(ForStatementInit::VariableDeclaration(vd)) = for_stmt.init.as_ref() {
            for decl in &vd.declarations {
                if let BindingPattern::BindingIdentifier(id) = &decl.id {
                    let name = id.name.as_str();
                    if let Some(&outer_val) = pre_init_scope.get(name) {
                        // This init variable shadowed an outer binding — restore the outer value.
                        scope.insert(name.to_string(), outer_val);
                    } else {
                        // Newly introduced by the init — remove from outer scope.
                        scope.remove(name);
                    }
                }
            }
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
        loops: &[(Option<BlockRef<'c, 'b>>, BlockRef<'c, 'b>, Vec<String>, Option<String>)],
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
        if let (Some(cb), Some(handler)) = (&catch_block, &try_stmt.handler) {
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
        if let (Some(fb), Some(finalizer)) = (&finally_block, &try_stmt.finalizer) {
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
        loops: &[(Option<BlockRef<'c, 'b>>, BlockRef<'c, 'b>, Vec<String>, Option<String>)],
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
        inner_loops.push((Some(update_block), exit_block, scope_keys.clone(), self.pending_label.take()));
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
        loops: &[(Option<BlockRef<'c, 'b>>, BlockRef<'c, 'b>, Vec<String>, Option<String>)],
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
        inner_loops.push((Some(update_block), exit_block, scope_keys.clone(), self.pending_label.take()));
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

    // ── Module-level const init function for imported files ────────────────

    /// Generate an `__init_module_N()` MLIR function that initializes all non-function
    /// module-level const declarations from an imported file and stores them as module globals.
    /// This function is called at the very start of `main`.
    pub(super) fn lower_imported_module_init(&mut self, program: &Program<'_>) -> Result<()> {
        use oxc_ast::ast::{ExportDefaultDeclarationKind, Declaration};

        // Collect the non-function const declarations (arrays, objects, literals, etc.)
        // We skip arrow functions and function expressions since those are handled by
        // lower_module_const_functions already.
        let mut has_init = false;
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
            if let Some(vd) = vd_opt {
                for decl in &vd.declarations {
                    if let Some(init) = &decl.init {
                        let inner = Lowerer::strip_ts_casts(init);
                        let is_fn = matches!(inner,
                            Expression::ArrowFunctionExpression(_) |
                            Expression::FunctionExpression(_)
                        );
                        if !is_fn {
                            has_init = true;
                            break;
                        }
                    }
                }
            }
            if has_init { break; }
        }
        if !has_init { return Ok(()); }

        let i32_type = self.i32_type();
        let fn_name = format!("__init_module_{}", self.module_init_fn_count);
        self.module_init_fn_count += 1;
        let fn_type = FunctionType::new(self.ctx, &[], &[]);

        let region = Region::new();
        let entry = region.append_block(Block::new(&[]));
        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();
        let mut current_block = entry;

        // Inject all currently known module globals so init code can reference them.
        let i64_type = self.i64_type();
        for global_name in self.module_global_names.clone() {
            if !scope.contains_key(&global_name) {
                let key_ptr = self.get_string_ptr(&global_name, current_block)?;
                let val: Value<'_, '_> = current_block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_get_module_global"),
                    &[key_ptr], &[i64_type], self.loc,
                )).result(0)?.into();
                scope.insert(global_name, val);
            }
        }

        for stmt in &program.body {
            let is_non_fn_var = match stmt {
                Statement::VariableDeclaration(vd) => {
                    vd.declarations.iter().any(|d| {
                        if let Some(init) = &d.init {
                            let inner = Lowerer::strip_ts_casts(init);
                            !matches!(inner,
                                Expression::ArrowFunctionExpression(_) |
                                Expression::FunctionExpression(_)
                            )
                        } else { false }
                    })
                }
                Statement::ExportNamedDeclaration(exp) => {
                    if let Some(Declaration::VariableDeclaration(vd)) = &exp.declaration {
                        vd.declarations.iter().any(|d| {
                            if let Some(init) = &d.init {
                                let inner = Lowerer::strip_ts_casts(init);
                                !matches!(inner,
                                    Expression::ArrowFunctionExpression(_) |
                                    Expression::FunctionExpression(_)
                                )
                            } else { false }
                        })
                    } else { false }
                }
                _ => false,
            };
            if !is_non_fn_var { continue; }

            // Lower the variable declaration (sets module globals via ts_set_module_global).
            let (_, nb) = self.lower_statement(stmt, current_block, &region, &mut scope, &[])?;
            current_block = nb;
        }

        // Release all loaded module globals (they were just borrowed for reading).
        for (name, v) in &scope {
            if self.module_global_names.contains(name) {
                let v_i64 = self.ensure_i64(*v, current_block)?;
                current_block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                    &[v_i64], &[], self.loc,
                ));
            }
        }

        current_block.append_operation(func::r#return(&[], self.loc));

        let op = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, &fn_name),
            TypeAttribute::new(fn_type.into()),
            region,
            &[],
            self.loc,
        );
        self.module.body().append_operation(op);
        self.funcs.insert(fn_name.clone(), FuncSig {
            param_types: vec![],
            return_type: None,
            has_rest: false,
            has_this_param: false,
        });
        self.module_init_fns.push(fn_name);
        Ok(())
    }

}
