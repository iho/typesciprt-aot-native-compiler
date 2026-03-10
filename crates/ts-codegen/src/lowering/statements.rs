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
                }
                Ok((val_opt, nb))
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
        let mut result = None;
        for declarator in &var_decl.declarations {
            let name = match &declarator.id {
                BindingPattern::BindingIdentifier(b) => b.name.to_string(),
                _ => { tracing::debug!("skipping non-simple binding pattern"); continue; }
            };
            if let Some(init) = &declarator.init {
                let (val_opt, nb) = self.lower_expression(init, block, region, scope)?;
                block = nb;
                if let Some(val) = val_opt {
                    scope.insert(name.clone(), val);
                    result = Some(val);
                }
            }
        }
        Ok((result, block))
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

        // ARC: Release all variables in the current scope before returning.
        for (_, v) in scope.iter() {
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
        let merge_arg_types: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            scope_keys.iter().map(|k| (scope[k].r#type(), self.loc)).collect();

        let then_block  = region.append_block(Block::new(&[]));
        let else_block  = region.append_block(Block::new(&[]));
        let merge_block = region.append_block(Block::new(&merge_arg_types));

        block.append_operation(cf::cond_br(
            self.ctx, cond_i1, &then_block, &else_block, &[], &[], self.loc,
        ));

        // Then branch
        let mut then_scope = scope.clone();
        let (_, then_end) = self.lower_statement(&if_stmt.consequent, then_block, region, &mut then_scope, loops)?;
        let then_vals: Vec<Value<'c, 'b>> =
            scope_keys.iter().map(|k| *then_scope.get(k).unwrap_or(&scope[k])).collect();
        self.terminate_with_br(then_end, &merge_block, &then_vals);

        // Else branch
        let mut else_scope = scope.clone();
        if let Some(alt) = &if_stmt.alternate {
            let (_, else_end) = self.lower_statement(alt, else_block, region, &mut else_scope, loops)?;
            let else_vals: Vec<Value<'c, 'b>> =
                scope_keys.iter().map(|k| *else_scope.get(k).unwrap_or(&scope[k])).collect();
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
        let phi_types: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            scope_keys.iter().map(|k| (scope[k].r#type(), self.loc)).collect();

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
        let body_vals: Vec<Value<'c, 'b>> =
            scope_keys.iter().map(|k| *body_scope.get(k).unwrap_or(&header_scope[k])).collect();
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
        let phi_types: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            scope_keys.iter().map(|k| (scope[k].r#type(), self.loc)).collect();

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
        let body_vals: Vec<Value<'c, 'b>> =
            scope_keys.iter().map(|k| *body_scope.get(k).unwrap_or(&header_scope[k])).collect();
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
        let update_vals: Vec<Value<'c, 'b>> =
            scope_keys.iter().map(|k| *update_scope.get(k).unwrap_or(&header_scope[k])).collect();
        self.terminate_with_br(update_block, &header_block, &update_vals);

        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), exit_block.argument(i)?.into());
        }

        Ok((None, exit_block))
    }

}
