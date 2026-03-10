use super::*;

impl<'c, 'm> Lowerer<'c, 'm> {

    // ── Binary expressions ────────────────────────────────────────────────

    pub(super) fn lower_binary_expression<'b>(
        &mut self,
        binop: &BinaryExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        use oxc_ast::ast::BinaryOperator;

        let (lhs_opt, nb) = self.lower_expression(&binop.left, block, region, scope)?;
        block = nb;
        let lhs = lhs_opt
            .ok_or_else(|| anyhow::anyhow!("binary op: no left value"))?;
        let (rhs_opt, nb) = self.lower_expression(&binop.right, block, region, scope)?;
        block = nb;
        let rhs = rhs_opt
            .ok_or_else(|| anyhow::anyhow!("binary op: no right value"))?;

        let lhs_i32 = self.ensure_i32(lhs, block)?;
        let rhs_i32 = self.ensure_i32(rhs, block)?;

        let op = match binop.operator {
            BinaryOperator::Addition       => arith::addi(lhs_i32, rhs_i32, self.loc),
            BinaryOperator::Subtraction    => arith::subi(lhs_i32, rhs_i32, self.loc),
            BinaryOperator::Multiplication => arith::muli(lhs_i32, rhs_i32, self.loc),
            BinaryOperator::Division       => arith::divsi(lhs_i32, rhs_i32, self.loc),
            BinaryOperator::LessThan         => arith::cmpi(self.ctx, arith::CmpiPredicate::Slt, lhs_i32, rhs_i32, self.loc),
            BinaryOperator::GreaterThan      => arith::cmpi(self.ctx, arith::CmpiPredicate::Sgt, lhs_i32, rhs_i32, self.loc),
            BinaryOperator::LessEqualThan    => arith::cmpi(self.ctx, arith::CmpiPredicate::Sle, lhs_i32, rhs_i32, self.loc),
            BinaryOperator::GreaterEqualThan => arith::cmpi(self.ctx, arith::CmpiPredicate::Sge, lhs_i32, rhs_i32, self.loc),
            BinaryOperator::Equality
            | BinaryOperator::StrictEquality   => arith::cmpi(self.ctx, arith::CmpiPredicate::Eq,  lhs_i32, rhs_i32, self.loc),
            BinaryOperator::Inequality
            | BinaryOperator::StrictInequality => arith::cmpi(self.ctx, arith::CmpiPredicate::Ne,  lhs_i32, rhs_i32, self.loc),
            _ => bail!("unsupported binary operator: {:?}", binop.operator),
        };
        let res = block.append_operation(op).result(0)?.into();

        // ARC: Release operands.
        let lhs_i64 = self.ensure_i64(lhs, block)?;
        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
        let rhs_i64 = self.ensure_i64(rhs, block)?;
        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64], &[], self.loc));

        Ok((Some(res), block))
    }


    // ── Logical expressions (&& / ||) ─────────────────────────────────────

    pub(super) fn lower_logical_expression<'b>(
        &mut self,
        logical: &LogicalExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        use oxc_ast::ast::LogicalOperator;

        let (lhs_opt, nb) = self.lower_expression(&logical.left, block, region, scope)?;
        block = nb;
        let lhs = lhs_opt
            .ok_or_else(|| anyhow::anyhow!("logical op: no left value"))?;
        let l = self.ensure_i1(lhs, block)?;

        let orig_scope = scope.clone();
        let scope_keys: Vec<String> = orig_scope.keys().cloned().collect();
        let mut merge_arg_types = vec![(self.i1_type(), self.loc)];
        for k in &scope_keys {
            merge_arg_types.push((orig_scope[k].r#type(), self.loc));
        }

        let merge_block = region.append_block(Block::new(&merge_arg_types));
        let rhs_block = region.append_block(Block::new(&[]));

        let orig_vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| orig_scope[k]).collect();

        match logical.operator {
            LogicalOperator::And => {
                let mut false_args = vec![l];
                false_args.extend(orig_vals.iter().copied());
                block.append_operation(cf::cond_br(self.ctx, l, &rhs_block, &merge_block, &[], &false_args, self.loc));
            }
            LogicalOperator::Or => {
                let mut true_args = vec![l];
                true_args.extend(orig_vals.iter().copied());
                block.append_operation(cf::cond_br(self.ctx, l, &merge_block, &rhs_block, &true_args, &[], self.loc));
            }
            _ => bail!("unsupported logical operator: {:?}", logical.operator),
        }

        let mut rhs_scope = orig_scope.clone();
        let (rhs_opt, nb) = self.lower_expression(&logical.right, rhs_block, region, &mut rhs_scope)?;
        let rhs_block = nb;
        let rhs = rhs_opt.ok_or_else(|| anyhow::anyhow!("logical op: no right value"))?;
        let r = self.ensure_i1(rhs, rhs_block)?;
        
        let mut rhs_end_args = vec![r];
        for k in &scope_keys {
            rhs_end_args.push(*rhs_scope.get(k).unwrap_or(&orig_scope[k]));
        }
        rhs_block.append_operation(cf::br(&merge_block, &rhs_end_args, self.loc));

        let res_i1 = merge_block.argument(0)?.into();
        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), merge_block.argument(i + 1)?.into());
        }

        Ok((Some(res_i1), merge_block))
    }

    // ── Conditional expression (? :) ──────────────────────────────────────

    pub(super) fn lower_conditional_expression<'b>(
        &mut self,
        cond: &oxc_ast::ast::ConditionalExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let (test_val_opt, nb) = self.lower_expression(&cond.test, block, region, scope)?;
        block = nb;
        let test_val = test_val_opt
            .ok_or_else(|| anyhow::anyhow!("conditional ?: no test value"))?;
        let test_i1 = self.ensure_i1(test_val, block)?;

        // For simplicity and matching current eager logical op behavior,
        // we eagerly evaluate both branches and use `arith::select`.
        // A true short-circuiting ?: would require a block split like `if/else`.
        let (cons_val_opt, nb) = self.lower_expression(&cond.consequent, block, region, scope)?;
        block = nb;
        let cons_val = cons_val_opt
            .ok_or_else(|| anyhow::anyhow!("conditional ?: no consequent value"))?;
        let (alt_val_opt, nb) = self.lower_expression(&cond.alternate, block, region, scope)?;
        block = nb;
        let alt_val = alt_val_opt
            .ok_or_else(|| anyhow::anyhow!("conditional ?: no alternate value"))?;

        // Ensure both branches return the same type (i32).
        let cons_i32 = self.ensure_i32(cons_val, block)?;
        let alt_i32 = self.ensure_i32(alt_val, block)?;

        let op = arith::select(test_i1, cons_i32, alt_i32, self.loc);
        Ok((Some(block.append_operation(op).result(0)?.into()), block))
    }

    // ── Unary expressions ─────────────────────────────────────────────────

    pub(super) fn lower_unary_expression<'b>(
        &mut self,
        unary: &UnaryExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        use oxc_ast::ast::UnaryOperator;

        let (operand_opt, nb) = self.lower_expression(&unary.argument, block, region, scope)?;
        block = nb;
        let operand = operand_opt
            .ok_or_else(|| anyhow::anyhow!("unary op: no operand"))?;

        let res = match unary.operator {
            UnaryOperator::UnaryNegation => {
                let zero = self.lower_numeric_literal(0, block)?;
                let op_val = self.ensure_i32(operand, block)?;
                block.append_operation(arith::subi(zero, op_val, self.loc)).result(0)?.into()
            }
            UnaryOperator::LogicalNot => {
                let x = self.ensure_i1(operand, block)?;
                let zero_i1 = block
                    .append_operation(arith::constant(
                        self.ctx,
                        IntegerAttribute::new(self.i1_type(), 0).into(),
                        self.loc,
                    ))
                    .result(0)?
                    .into();
                block.append_operation(arith::cmpi(self.ctx, arith::CmpiPredicate::Eq, x, zero_i1, self.loc)).result(0)?.into()
            }
            _ => bail!("unsupported unary operator: {:?}", unary.operator),
        };

        // ARC: Release operand.
        let operand_i64 = self.ensure_i64(operand, block)?;
        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[operand_i64], &[], self.loc));

        Ok((Some(res), block))
    }


    // ── Update expressions (i++, ++i, i--, --i) ───────────────────────────

    pub(super) fn lower_update_expression<'b>(
        &mut self,
        update: &oxc_ast::ast::UpdateExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        use oxc_ast::ast::UpdateOperator;

        let id = match &update.argument {
            oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => id,
            _ => bail!("update expression: only simple identifiers are supported"),
        };
        let name = id.name.to_string();

        // ARC: Manual identifier read.
        let name = id.name.to_string();
        let old_val = *scope.get(&name).ok_or_else(|| anyhow::anyhow!("undefined: {}", name))?;
        let old_i64 = self.ensure_i64(old_val, block)?;
        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"), &[old_i64], &[], self.loc));
        
        
        let old_val_i32 = self.ensure_i32(old_val, block)?;
        let one = self.lower_numeric_literal(1, block)?;

        let new_val_i32: Value<'c, 'b> = match update.operator {
            UpdateOperator::Increment => block.append_operation(arith::addi(old_val_i32, one, self.loc)).result(0)?.into(),
            UpdateOperator::Decrement => block.append_operation(arith::subi(old_val_i32, one, self.loc)).result(0)?.into(),
        };

        // ARC: Release old value in scope.
        if let Some(&v) = scope.get(&name) {
             let v_i64 = self.ensure_i64(v, block)?;
             block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[v_i64], &[], self.loc));
        }
        
        // Box new value and store in scope.
        let new_val_boxed = self.ensure_i64(new_val_i32, block)?;
        scope.insert(name.clone(), new_val_boxed);

        if update.prefix {
            // ARC: Return owned new_val.
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"), &[new_val_boxed], &[], self.loc));
            
            // Cleanup: release old_val (from lower_expression)
            let old_i64 = self.ensure_i64(old_val, block)?;
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[old_i64], &[], self.loc));
            
            Ok((Some(new_val_boxed), block))
        } else {
            // Postfix: return owned old_val.
            Ok((Some(old_val), block))
        }
    }


    // ── Assignment expressions ────────────────────────────────────────────

    pub(super) fn lower_assignment_expression<'b>(
        &mut self,
        assign: &oxc_ast::ast::AssignmentExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        use oxc_ast::ast::AssignmentOperator;

        let (rhs_opt, nb) = self.lower_expression(&assign.right, block, region, scope)?;
        block = nb;
        let rhs = rhs_opt
            .ok_or_else(|| anyhow::anyhow!("assignment: rhs produced no value"))?;

        match &assign.left {
            AssignmentTarget::AssignmentTargetIdentifier(id) => {
                let name = id.name.to_string();
                
                // ARC: Get the new value (possibly compound).
                let new_val = match assign.operator {
                    AssignmentOperator::Assign => rhs,
                    _ => {
                        // ARC: Manual identifier read (equivalent to lower_expression(Identifier)).
                        let name = id.name.to_string();
                        let lhs = *scope.get(&name).ok_or_else(|| anyhow::anyhow!("undefined: {}", name))?;
                        let lhs_i64 = self.ensure_i64(lhs, block)?;
                        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"), &[lhs_i64], &[], self.loc));
                        
                        
                        let lhs_i32 = self.ensure_i32(lhs, block)?;
                        let rhs_i32 = self.ensure_i32(rhs, block)?;
                        let res_i32 = match assign.operator {
                            AssignmentOperator::Addition => block.append_operation(arith::addi(lhs_i32, rhs_i32, self.loc)).result(0)?.into(),
                            AssignmentOperator::Subtraction => block.append_operation(arith::subi(lhs_i32, rhs_i32, self.loc)).result(0)?.into(),
                            AssignmentOperator::Multiplication => block.append_operation(arith::muli(lhs_i32, rhs_i32, self.loc)).result(0)?.into(),
                            _ => bail!("unsupported compound assignment operator"),
                        };
                        
                        // ARC: Release operands of the compound operation.
                        let lhs_i64 = self.ensure_i64(lhs, block)?;
                        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
                        let rhs_i64 = self.ensure_i64(rhs, block)?;
                        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64], &[], self.loc));
                        
                        res_i32
                    }
                };
                
                // ARC: Release the old value in the scope.
                if let Some(&old_val) = scope.get(&name) {
                    let old_i64 = self.ensure_i64(old_val, block)?;
                    block.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[old_i64],
                        &[],
                        self.loc,
                    ));
                }
                
                scope.insert(name, new_val);
                
                // ARC: Return an OWNED reference. 
                let new_i64 = self.ensure_i64(new_val, block)?;
                block.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"),
                    &[new_i64],
                    &[],
                    self.loc,
                ));
                
                Ok((Some(new_val), block))
            }
            AssignmentTarget::StaticMemberExpression(m) => {
                self.lower_static_member_assignment(&**m, assign.operator, rhs, block, region, scope)
            }
            AssignmentTarget::ComputedMemberExpression(m) => {
                self.lower_computed_member_assignment(&**m, assign.operator, rhs, block, region, scope)
            }
            _ => bail!("unsupported assignment target: {:?}", assign.left),
        }
    }


    fn lower_static_member_assignment<'b>(
        &mut self,
        member: &oxc_ast::ast::StaticMemberExpression<'_>,
        operator: oxc_ast::ast::AssignmentOperator,
        rhs: Value<'c, 'b>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        use oxc_ast::ast::AssignmentOperator;
        if operator != AssignmentOperator::Assign {
            bail!("compound assignment to member expressions is not supported yet");
        }
        let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
        block = nb;
        let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("member assignment: object produced no value"))?;
        let obj_i64 = self.ensure_i64(obj, block)?;
        
        let key_ptr = self.lower_string_literal(&member.property.name, block)?;
        let val_i64 = self.ensure_i64(rhs, block)?;

        // ts_obj_set(obj, key, val)
        block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
            &[obj_i64, key_ptr, val_i64],
            &[],
            self.loc,
        ));
        
        // ARC: release obj after ts_obj_set
        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
        
        Ok((Some(rhs), block))
    }

    fn lower_computed_member_assignment<'b>(
        &mut self,
        member: &oxc_ast::ast::ComputedMemberExpression<'_>,
        operator: oxc_ast::ast::AssignmentOperator,
        rhs: Value<'c, 'b>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        use oxc_ast::ast::AssignmentOperator;
        if operator != AssignmentOperator::Assign {
            bail!("compound assignment to computed member expressions is not supported yet");
        }
        let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
        block = nb;
        let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("computed assignment: object produced no value"))?;
        let obj_i64 = self.ensure_i64(obj, block)?;

        let (idx_opt, nb) = self.lower_expression(&member.expression, block, region, scope)?;
        block = nb;
        let idx = idx_opt.ok_or_else(|| anyhow::anyhow!("computed assignment: index produced no value"))?;
        let idx_i32 = self.ensure_i32(idx, block)?;

        let val_i64 = self.ensure_i64(rhs, block)?;

        // ts_arr_set(obj, idx, val)
        block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_set"),
            &[obj_i64, idx_i32, val_i64],
            &[],
            self.loc,
        ));

        // ARC: release obj and idx after ts_arr_set.
        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
        let idx_i64 = self.ensure_i64(idx, block)?;
        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[idx_i64], &[], self.loc));

        Ok((Some(rhs), block))
    }
}

