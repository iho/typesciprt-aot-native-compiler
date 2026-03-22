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

        // `instanceof` — rhs is always a class-name identifier, not a runtime value;
        // lower only the lhs so we don't error on "undefined variable: Dog".
        if binop.operator == BinaryOperator::Instanceof {
            return self.lower_instanceof(binop, block, region, scope);
        }

        // `key in obj` — uses ts_val_has_key(obj, key)
        if binop.operator == BinaryOperator::In {
            let (key_opt, nb) = self.lower_expression(&binop.left, block, region, scope)?;
            block = nb;
            let key = self.ensure_i64(key_opt.ok_or_else(|| anyhow::anyhow!("in: no key"))?, block)?;
            let (obj_opt, nb) = self.lower_expression(&binop.right, block, region, scope)?;
            block = nb;
            let obj = self.ensure_i64(obj_opt.ok_or_else(|| anyhow::anyhow!("in: no obj"))?, block)?;
            let res: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_has_key"),
                &[obj, key], &[self.i64_type()], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[key], &[], self.loc));
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj], &[], self.loc));
            return Ok((Some(res), block));
        }

        let (lhs_opt, nb) = self.lower_expression(&binop.left, block, region, scope)?;
        block = nb;
        let lhs = lhs_opt
            .ok_or_else(|| anyhow::anyhow!("binary op: no left value"))?;
        let (rhs_opt, nb) = self.lower_expression(&binop.right, block, region, scope)?;
        block = nb;
        let rhs = rhs_opt
            .ok_or_else(|| anyhow::anyhow!("binary op: no right value"))?;

        // Polymorphic + : runtime dispatch via ts_add (integer add or string concat).
        if binop.operator == BinaryOperator::Addition
            && (lhs.r#type() == self.i64_type() || rhs.r#type() == self.i64_type())
        {
            let lhs_i64 = self.ensure_i64(lhs, block)?;
            let rhs_i64 = self.ensure_i64(rhs, block)?;
            let res: Value<'c, 'b> = block
                .append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_add"),
                    &[lhs_i64, rhs_i64],
                    &[self.i64_type()],
                    self.loc,
                ))
                .result(0)?.into();
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64], &[], self.loc));
            return Ok((Some(res), block));
        }

        // When either operand is i64 (heap or float), use runtime dispatch for all ops
        // (equality, arithmetic, comparison) to correctly handle floats and strings.
        if lhs.r#type() == self.i64_type() || rhs.r#type() == self.i64_type() {
            let lhs_i64 = self.ensure_i64(lhs, block)?;
            let rhs_i64 = self.ensure_i64(rhs, block)?;

            let is_eq_op = matches!(binop.operator,
                BinaryOperator::Equality | BinaryOperator::StrictEquality |
                BinaryOperator::Inequality | BinaryOperator::StrictInequality);

            let is_cmp_op = matches!(binop.operator,
                BinaryOperator::LessThan | BinaryOperator::GreaterThan |
                BinaryOperator::LessEqualThan | BinaryOperator::GreaterEqualThan);

            let is_arith_op = matches!(binop.operator,
                BinaryOperator::Subtraction | BinaryOperator::Multiplication |
                BinaryOperator::Division | BinaryOperator::Remainder |
                BinaryOperator::Exponential | BinaryOperator::BitwiseOR |
                BinaryOperator::BitwiseAnd | BinaryOperator::BitwiseXOR |
                BinaryOperator::ShiftLeft | BinaryOperator::ShiftRight |
                BinaryOperator::ShiftRightZeroFill);

            let result_opt: Option<Value<'c, 'b>> = if is_eq_op {
                let eq_i32: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_strict_eq"),
                    &[lhs_i64, rhs_i64], &[self.i32_type()], self.loc,
                )).result(0)?.into();
                let zero: Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx, IntegerAttribute::new(self.i32_type(), 0).into(), self.loc,
                )).result(0)?.into();
                let pred = if matches!(binop.operator,
                    BinaryOperator::Inequality | BinaryOperator::StrictInequality)
                { arith::CmpiPredicate::Eq } else { arith::CmpiPredicate::Ne };
                Some(block.append_operation(arith::cmpi(self.ctx, pred, eq_i32, zero, self.loc)).result(0)?.into())
            } else if is_cmp_op {
                let fn_name = match binop.operator {
                    BinaryOperator::LessThan         => "ts_lt",
                    BinaryOperator::GreaterThan      => "ts_gt",
                    BinaryOperator::LessEqualThan    => "ts_le",
                    BinaryOperator::GreaterEqualThan => "ts_ge",
                    _ => unreachable!(),
                };
                let cmp_i32: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, fn_name),
                    &[lhs_i64, rhs_i64], &[self.i32_type()], self.loc,
                )).result(0)?.into();
                Some(self.ensure_i1(cmp_i32, block)?)
            } else if is_arith_op {
                let fn_name = match binop.operator {
                    BinaryOperator::Subtraction       => "ts_sub",
                    BinaryOperator::Multiplication    => "ts_mul",
                    BinaryOperator::Division          => "ts_div",
                    BinaryOperator::Remainder         => "ts_mod",
                    BinaryOperator::Exponential       => "ts_pow",
                    BinaryOperator::BitwiseOR         => "ts_bitor",
                    BinaryOperator::BitwiseAnd        => "ts_bitand",
                    BinaryOperator::BitwiseXOR        => "ts_bitxor",
                    BinaryOperator::ShiftLeft         => "ts_shl",
                    BinaryOperator::ShiftRight        => "ts_shr",
                    BinaryOperator::ShiftRightZeroFill => "ts_ushr",
                    _ => unreachable!(),
                };
                Some(block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, fn_name),
                    &[lhs_i64, rhs_i64], &[self.i64_type()], self.loc,
                )).result(0)?.into())
            } else {
                None
            };

            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64], &[], self.loc));

            if result_opt.is_some() || is_eq_op || is_cmp_op || is_arith_op {
                return Ok((result_opt, block));
            }
            // Fall through for other operators (bitwise etc.) using i32 path below.
        }

        // For ** and bitwise operators with i32 operands, route through runtime (returns i64).
        let is_runtime_op = matches!(binop.operator,
            BinaryOperator::Exponential | BinaryOperator::BitwiseOR |
            BinaryOperator::BitwiseAnd | BinaryOperator::BitwiseXOR |
            BinaryOperator::ShiftLeft | BinaryOperator::ShiftRight |
            BinaryOperator::ShiftRightZeroFill);
        if is_runtime_op {
            let lhs_i64 = self.ensure_i64(lhs, block)?;
            let rhs_i64 = self.ensure_i64(rhs, block)?;
            let fn_name = match binop.operator {
                BinaryOperator::Exponential        => "ts_pow",
                BinaryOperator::BitwiseOR          => "ts_bitor",
                BinaryOperator::BitwiseAnd         => "ts_bitand",
                BinaryOperator::BitwiseXOR         => "ts_bitxor",
                BinaryOperator::ShiftLeft          => "ts_shl",
                BinaryOperator::ShiftRight         => "ts_shr",
                BinaryOperator::ShiftRightZeroFill => "ts_ushr",
                _ => unreachable!(),
            };
            let res: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, fn_name),
                &[lhs_i64, rhs_i64], &[self.i64_type()], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64], &[], self.loc));
            return Ok((Some(res), block));
        }

        let lhs_i32 = self.ensure_i32(lhs, block)?;
        let rhs_i32 = self.ensure_i32(rhs, block)?;

        let op = match binop.operator {
            BinaryOperator::Addition       => arith::addi(lhs_i32, rhs_i32, self.loc),
            BinaryOperator::Subtraction    => arith::subi(lhs_i32, rhs_i32, self.loc),
            BinaryOperator::Multiplication => arith::muli(lhs_i32, rhs_i32, self.loc),
            BinaryOperator::Division       => arith::divsi(lhs_i32, rhs_i32, self.loc),
            BinaryOperator::Remainder      => arith::remsi(lhs_i32, rhs_i32, self.loc),
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

    // ── instanceof ────────────────────────────────────────────────────────

    fn lower_instanceof<'b>(
        &mut self,
        binop: &BinaryExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        // If RHS is not a simple identifier (e.g. `x instanceof options.type`), evaluate both
        // sides for side-effects and return false — we can't resolve the class at compile time.
        let Expression::Identifier(class_id) = &binop.right else {
            let (lhs_opt, nb) = self.lower_expression(&binop.left, block, region, scope)?;
            block = nb;
            let (rhs_opt, nb2) = self.lower_expression(&binop.right, block, region, scope)?;
            block = nb2;
            if let Some(v) = lhs_opt {
                let v64 = self.ensure_i64(v, block)?;
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[v64], &[], self.loc));
            }
            if let Some(v) = rhs_opt {
                let v64 = self.ensure_i64(v, block)?;
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[v64], &[], self.loc));
            }
            let false_val: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FFA_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            return Ok((Some(false_val), block));
        };
        let target = class_id.name.to_string();

        // Lower lhs only (rhs is a compile-time class name).
        let (lhs_opt, nb) = self.lower_expression(&binop.left, block, region, scope)?;
        block = nb;
        let lhs = lhs_opt.ok_or_else(|| anyhow::anyhow!("instanceof: lhs produced no value"))?;
        let lhs_i64 = self.ensure_i64(lhs, block)?;

        // Read __class__ string from the object.
        let class_key_ptr = self.get_string_ptr("__class__", block)?;
        let class_val: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
            &[lhs_i64, class_key_ptr],
            &[self.i64_type()],
            self.loc,
        )).result(0)?.into();
        block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[lhs_i64], &[], self.loc,
        ));

        // Build the set of class names that satisfy `instanceof target`
        // (target itself + all classes that transitively extend target).
        let matching: Vec<String> = {
            let all: Vec<String> = self.classes.keys().cloned().collect();
            let mut m: Vec<String> = all.into_iter()
                .filter(|n| self.is_subclass_of(n, &target))
                .collect();
            // If target is unknown (e.g. built-in), still check it directly.
            if m.is_empty() { m.push(target.clone()); }
            m
        };

        // OR-chain: result |= (class_val === "ClassName") for each matching name.
        let zero_i32: Value<'c, 'b> = block.append_operation(arith::constant(
            self.ctx, IntegerAttribute::new(self.i32_type(), 0).into(), self.loc,
        )).result(0)?.into();
        let mut result_i32 = zero_i32;

        for class_name in &matching {
            let target_str = self.lower_string_literal(class_name, block)?;
            let eq: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_val_strict_eq"),
                &[class_val, target_str],
                &[self.i32_type()],
                self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[target_str], &[], self.loc,
            ));
            result_i32 = block.append_operation(
                arith::ori(result_i32, eq, self.loc)
            ).result(0)?.into();
        }

        block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[class_val], &[], self.loc,
        ));

        // Convert i32 result to i1 boolean.
        let is_instance: Value<'c, 'b> = block.append_operation(arith::cmpi(
            self.ctx, arith::CmpiPredicate::Ne, result_i32, zero_i32, self.loc,
        )).result(0)?.into();

        Ok((Some(is_instance), block))
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

        // ?? (nullish coalescing) needs different merge type (i64 instead of i1).
        if logical.operator == LogicalOperator::Coalesce {
            return self.lower_nullish_coalescing(logical, block, region, scope);
        }

        // JS semantics: `a && b` returns `a` if falsy, else `b`.
        //               `a || b` returns `a` if truthy, else `b`.
        // So we return the actual i64 value, not a boolean i1.
        let i64_type = self.i64_type();

        let (lhs_opt, nb) = self.lower_expression(&logical.left, block, region, scope)?;
        block = nb;
        let lhs = lhs_opt
            .ok_or_else(|| anyhow::anyhow!("logical op: no left value"))?;
        let lhs_i64 = self.ensure_i64(lhs, block)?;
        let l = self.ensure_i1(lhs_i64, block)?;

        // Normalize all scope vars to i64 before creating merge block.
        let scope_keys: Vec<String> = scope.keys().cloned().collect();
        for k in &scope_keys {
            let v64 = self.ensure_i64(scope[k], block)?;
            scope.insert(k.clone(), v64);
        }
        let orig_scope = scope.clone();

        // merge_block receives: (result_i64, ...scope_vals all i64)
        let mut merge_arg_types = vec![(i64_type, self.loc)];
        for _ in &scope_keys {
            merge_arg_types.push((i64_type, self.loc));
        }

        let merge_block = region.append_block(Block::new(&merge_arg_types));
        let rhs_block = region.append_block(Block::new(&[]));

        let orig_vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| orig_scope[k]).collect();

        match logical.operator {
            LogicalOperator::And => {
                // false → skip rhs, return lhs; true → evaluate rhs
                let mut false_args = vec![lhs_i64];
                false_args.extend(orig_vals.iter().copied());
                block.append_operation(cf::cond_br(self.ctx, l, &rhs_block, &merge_block, &[], &false_args, self.loc));
            }
            LogicalOperator::Or => {
                // true → return lhs; false → evaluate rhs
                let mut true_args = vec![lhs_i64];
                true_args.extend(orig_vals.iter().copied());
                block.append_operation(cf::cond_br(self.ctx, l, &merge_block, &rhs_block, &true_args, &[], self.loc));
            }
            _ => bail!("unsupported logical operator: {:?}", logical.operator),
        }

        // Release lhs in rhs_block: when we arrive here, lhs was NOT selected as the result
        // (falsy for ||, truthy for &&) — release the owned ref we no longer need.
        rhs_block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[lhs_i64],
            &[],
            self.loc,
        ));

        let mut rhs_scope = orig_scope.clone();
        let (rhs_opt, nb) = self.lower_expression(&logical.right, rhs_block, region, &mut rhs_scope)?;
        let rhs_block = nb;
        let rhs = rhs_opt.ok_or_else(|| anyhow::anyhow!("logical op: no right value"))?;
        let rhs_i64 = self.ensure_i64(rhs, rhs_block)?;

        let mut rhs_end_args = vec![rhs_i64];
        for k in &scope_keys {
            let v = *rhs_scope.get(k).unwrap_or(&orig_scope[k]);
            let v64 = self.ensure_i64(v, rhs_block).unwrap_or(v);
            rhs_end_args.push(v64);
        }
        rhs_block.append_operation(cf::br(&merge_block, &rhs_end_args, self.loc));

        let result_i64: Value<'c, 'b> = merge_block.argument(0)?.into();
        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), merge_block.argument(i + 1)?.into());
        }

        Ok((Some(result_i64), merge_block))
    }

    // ── Nullish coalescing (??) ────────────────────────────────────────────

    fn lower_nullish_coalescing<'b>(
        &mut self,
        logical: &LogicalExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        // Evaluate lhs.
        let (lhs_opt, nb) = self.lower_expression(&logical.left, block, region, scope)?;
        block = nb;
        let lhs = lhs_opt.ok_or_else(|| anyhow::anyhow!("??: no left value"))?;
        let lhs_i64 = self.ensure_i64(lhs, block)?;

        // Check if lhs is null or undefined.
        let is_null: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_is_nullish"),
            &[lhs_i64],
            &[self.i32_type()],
            self.loc,
        )).result(0)?.into();
        let is_null_i1 = self.ensure_i1(is_null, block)?;

        // Normalize all scope vars to i64 before creating merge block.
        let scope_keys: Vec<String> = scope.keys().cloned().collect();
        for k in &scope_keys {
            let v64 = self.ensure_i64(scope[k], block)?;
            scope.insert(k.clone(), v64);
        }
        let orig_scope = scope.clone();

        // merge_block: receives (i64 result, ...scope_vals all i64)
        let mut merge_arg_types = vec![(self.i64_type(), self.loc)];
        for _ in &scope_keys {
            merge_arg_types.push((self.i64_type(), self.loc));
        }
        let merge_block = region.append_block(Block::new(&merge_arg_types));
        let rhs_block   = region.append_block(Block::new(&[]));

        let orig_vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| orig_scope[k]).collect();

        // When lhs IS nullish → rhs_block (release lhs there, evaluate rhs)
        // When lhs is NOT nullish → merge_block with lhs_i64
        let mut not_null_args = vec![lhs_i64];
        not_null_args.extend(orig_vals.iter().copied());
        block.append_operation(cf::cond_br(
            self.ctx, is_null_i1,
            &rhs_block, &merge_block,
            &[],            // rhs_block args (none)
            &not_null_args, // merge_block args: [lhs_i64, ...scope_vals]
            self.loc,
        ));

        // rhs_block: lhs was nullish, release it and return rhs instead.
        rhs_block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[lhs_i64], &[], self.loc,
        ));
        let mut rhs_scope = orig_scope.clone();
        let (rhs_opt, nb) = self.lower_expression(&logical.right, rhs_block, region, &mut rhs_scope)?;
        let rhs_block = nb;
        let rhs = rhs_opt.ok_or_else(|| anyhow::anyhow!("??: no right value"))?;
        let rhs_i64 = self.ensure_i64(rhs, rhs_block)?;

        let mut rhs_args = vec![rhs_i64];
        for k in &scope_keys {
            let v = *rhs_scope.get(k).unwrap_or(&orig_scope[k]);
            let v64 = self.ensure_i64(v, rhs_block).unwrap_or(v);
            rhs_args.push(v64);
        }
        rhs_block.append_operation(cf::br(&merge_block, &rhs_args, self.loc));

        // Update scope from merge_block arguments.
        let result: Value<'c, 'b> = merge_block.argument(0)?.into();
        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), merge_block.argument(i + 1)?.into());
        }

        Ok((Some(result), merge_block))
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

        let i64t = self.i64_type();
        let scope_keys: Vec<String> = scope.keys().cloned().collect();

        // Normalise all scope values to i64 before branching (phi-node requirement).
        for k in &scope_keys {
            let v64 = self.ensure_i64(scope[k], block).unwrap_or(scope[k]);
            scope.insert(k.clone(), v64);
        }

        // merge_block: arg[0] = ternary result (i64), arg[1..] = scope vars (i64 each).
        let mut merge_types = vec![(i64t, self.loc)];
        merge_types.extend(scope_keys.iter().map(|_| (i64t, self.loc)));
        let merge_block = region.append_block(Block::new(&merge_types));

        let then_block = region.append_block(Block::new(&[]));
        let else_block = region.append_block(Block::new(&[]));

        block.append_operation(cf::cond_br(
            self.ctx, test_i1, &then_block, &else_block, &[], &[], self.loc,
        ));

        // Consequent (then) branch — only executed when test is truthy.
        let mut then_scope = scope.clone();
        let (cons_val_opt, then_end) =
            self.lower_expression(&cond.consequent, then_block, region, &mut then_scope)?;
        let cons_val = cons_val_opt
            .ok_or_else(|| anyhow::anyhow!("conditional ?: no consequent value"))?;
        let cons_i64 = self.ensure_i64(cons_val, then_end)?;
        let mut then_args = vec![cons_i64];
        for k in &scope_keys {
            let v = *then_scope.get(k).unwrap_or(&scope[k]);
            then_args.push(self.ensure_i64(v, then_end).unwrap_or(v));
        }
        self.terminate_with_br(then_end, &merge_block, &then_args);

        // Alternate (else) branch — only executed when test is falsy.
        let mut else_scope = scope.clone();
        let (alt_val_opt, else_end) =
            self.lower_expression(&cond.alternate, else_block, region, &mut else_scope)?;
        let alt_val = alt_val_opt
            .ok_or_else(|| anyhow::anyhow!("conditional ?: no alternate value"))?;
        let alt_i64 = self.ensure_i64(alt_val, else_end)?;
        let mut else_args = vec![alt_i64];
        for k in &scope_keys {
            let v = *else_scope.get(k).unwrap_or(&scope[k]);
            else_args.push(self.ensure_i64(v, else_end).unwrap_or(v));
        }
        self.terminate_with_br(else_end, &merge_block, &else_args);

        // Update scope to use merge-block phi arguments (skip index 0 = result).
        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), merge_block.argument(i + 1)?.into());
        }

        Ok((Some(merge_block.argument(0)?.into()), merge_block))
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
        use oxc_ast::ast::Expression;

        // `delete obj[key]` / `delete obj.prop` — special case: do not evaluate operand.
        if unary.operator == UnaryOperator::Delete {
            let i64t = self.i64_type();
            let result = match &unary.argument {
                Expression::ComputedMemberExpression(member) => {
                    let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
                    block = nb;
                    let (key_opt, nb) = self.lower_expression(&member.expression, block, region, scope)?;
                    block = nb;
                    let obj_i64 = self.ensure_i64(obj_opt.ok_or_else(|| anyhow::anyhow!("delete: obj no value"))?, block)?;
                    let key_i64 = self.ensure_i64(key_opt.ok_or_else(|| anyhow::anyhow!("delete: key no value"))?, block)?;
                    let r = block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_delete_key"),
                        &[obj_i64, key_i64], &[i64t], self.loc,
                    )).result(0)?.into();
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[key_i64], &[], self.loc));
                    r
                }
                Expression::StaticMemberExpression(member) => {
                    let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
                    block = nb;
                    let obj_i64 = self.ensure_i64(obj_opt.ok_or_else(|| anyhow::anyhow!("delete: obj no value"))?, block)?;
                    let prop = member.property.name.as_str();
                    let key_ptr = self.get_string_ptr(prop, block)?;
                    let r = block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_delete"),
                        &[obj_i64, key_ptr], &[i64t], self.loc,
                    )).result(0)?.into();
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                    r
                }
                _ => {
                    // For anything else, just return true (delete always succeeds in non-strict mode).
                    block.append_operation(arith::constant(
                        self.ctx, IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0001u64 as i64).into(), self.loc,
                    )).result(0)?.into()
                }
            };
            return Ok((Some(result), block));
        }

        let (operand_opt, nb) = self.lower_expression(&unary.argument, block, region, scope)?;
        block = nb;
        let operand = operand_opt
            .ok_or_else(|| anyhow::anyhow!("unary op: no operand"))?;

        // `typeof` — handled before the generic ARC-release path below.
        if unary.operator == UnaryOperator::Typeof {
            let val_i64 = self.ensure_i64(operand, block)?;
            let result: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_typeof"),
                &[val_i64],
                &[self.i64_type()],
                self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[val_i64], &[], self.loc,
            ));
            return Ok((Some(result), block));
        }

        let res = match unary.operator {
            UnaryOperator::UnaryNegation => {
                let zero = self.lower_numeric_literal(0, block)?;
                let op_val = self.ensure_i32(operand, block)?;
                block.append_operation(arith::subi(zero, op_val, self.loc)).result(0)?.into()
            }
            UnaryOperator::UnaryPlus => {
                // +x — coerce to number; for now just pass through (already a number-ish value).
                let i64t = self.i64_type();
                let op_i64 = self.ensure_i64(operand, block)?;
                block.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_coerce_number"),
                    &[op_i64], &[i64t], self.loc,
                )).result(0)?.into()
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
            UnaryOperator::Void => {
                // `void expr` — evaluate for side effects, return undefined.
                // The operand is already evaluated above. Release it, then return undefined.
                let i64t = self.i64_type();
                let operand_i64 = self.ensure_i64(operand, block)?;
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[operand_i64], &[], self.loc));
                let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx,
                    IntegerAttribute::new(i64t, 0x7FF8_0000_0000_0000u64 as i64).into(),
                    self.loc,
                )).result(0)?.into();
                return Ok((Some(undef), block));
            }
            UnaryOperator::BitwiseNot => {
                let op_i64 = self.ensure_i64(operand, block)?;
                let res: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_bitnot"),
                    &[op_i64], &[self.i64_type()], self.loc,
                )).result(0)?.into();
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[op_i64], &[], self.loc));
                return Ok((Some(res), block));
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
        _region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        use oxc_ast::ast::UpdateOperator;

        // Handle member expression updates: obj.prop++ / obj.#prop++
        match &update.argument {
            oxc_ast::ast::SimpleAssignmentTarget::StaticMemberExpression(m) => {
                // Static class field: ClassName.field++ → module global read/write
                if let Expression::Identifier(id) = &m.object {
                    let class_name = id.name.as_str();
                    let field_name = m.property.name.as_str();
                    let is_static_field = self.classes.get(class_name)
                        .map(|sig| sig.static_fields.contains(field_name))
                        .unwrap_or(false);
                    if is_static_field {
                        let global_key = format!("__static_{}_{}", class_name, field_name);
                        let key_ptr = self.get_string_ptr(&global_key, block)?;
                        let i64t = self.i64_type();
                        let old_i64: Value<'c, 'b> = block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_get_module_global"),
                            &[key_ptr], &[i64t], self.loc,
                        )).result(0)?.into();
                        let old_i32 = self.ensure_i32(old_i64, block)?;
                        let one = self.lower_numeric_literal(1, block)?;
                        let new_i32: Value<'c, 'b> = match update.operator {
                            UpdateOperator::Increment => block.append_operation(arith::addi(old_i32, one, self.loc)).result(0)?.into(),
                            UpdateOperator::Decrement => block.append_operation(arith::subi(old_i32, one, self.loc)).result(0)?.into(),
                        };
                        let new_i64 = self.ensure_i64(new_i32, block)?;
                        let key_ptr2 = self.get_string_ptr(&global_key, block)?;
                        block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_set_module_global"),
                            &[key_ptr2, new_i64], &[], self.loc,
                        ));
                        if update.prefix {
                            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[old_i64], &[], self.loc));
                            return Ok((Some(new_i64), block));
                        } else {
                            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[new_i64], &[], self.loc));
                            return Ok((Some(old_i64), block));
                        }
                    }
                }
                let (obj_opt, nb) = self.lower_expression(&m.object, block, _region, scope)?;
                block = nb;
                let obj_val = obj_opt.ok_or_else(|| anyhow::anyhow!("update member: object produced no value"))?;
                let obj_i64 = self.ensure_i64(obj_val, block)?;
                let key_ptr = self.get_string_ptr(m.property.name.as_str(), block)?;
                let i64t = self.i64_type();
                // Load old value
                let old_i64: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                    &[obj_i64, key_ptr], &[i64t], self.loc,
                )).result(0)?.into();
                // Compute new value
                let old_i32 = self.ensure_i32(old_i64, block)?;
                let one = self.lower_numeric_literal(1, block)?;
                let new_i32: Value<'c, 'b> = match update.operator {
                    UpdateOperator::Increment => block.append_operation(arith::addi(old_i32, one, self.loc)).result(0)?.into(),
                    UpdateOperator::Decrement => block.append_operation(arith::subi(old_i32, one, self.loc)).result(0)?.into(),
                };
                let new_i64 = self.ensure_i64(new_i32, block)?;
                // Store new value
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
                    &[obj_i64, key_ptr, new_i64], &[], self.loc,
                ));
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                if update.prefix {
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[old_i64], &[], self.loc));
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"), &[new_i64], &[], self.loc));
                    return Ok((Some(new_i64), block));
                } else {
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[new_i64], &[], self.loc));
                    return Ok((Some(old_i64), block));
                }
            }
            oxc_ast::ast::SimpleAssignmentTarget::PrivateFieldExpression(m) => {
                let (obj_opt, nb) = self.lower_expression(&m.object, block, _region, scope)?;
                block = nb;
                let obj_val = obj_opt.ok_or_else(|| anyhow::anyhow!("update private field: object produced no value"))?;
                let obj_i64 = self.ensure_i64(obj_val, block)?;
                let key_name = format!("__priv_{}", m.field.name.as_str());
                let key_ptr = self.get_string_ptr(&key_name, block)?;
                let i64t = self.i64_type();
                let old_i64: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                    &[obj_i64, key_ptr], &[i64t], self.loc,
                )).result(0)?.into();
                let old_i32 = self.ensure_i32(old_i64, block)?;
                let one = self.lower_numeric_literal(1, block)?;
                let new_i32: Value<'c, 'b> = match update.operator {
                    UpdateOperator::Increment => block.append_operation(arith::addi(old_i32, one, self.loc)).result(0)?.into(),
                    UpdateOperator::Decrement => block.append_operation(arith::subi(old_i32, one, self.loc)).result(0)?.into(),
                };
                let new_i64 = self.ensure_i64(new_i32, block)?;
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
                    &[obj_i64, key_ptr, new_i64], &[], self.loc,
                ));
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                if update.prefix {
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[old_i64], &[], self.loc));
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"), &[new_i64], &[], self.loc));
                    return Ok((Some(new_i64), block));
                } else {
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[new_i64], &[], self.loc));
                    return Ok((Some(old_i64), block));
                }
            }
            oxc_ast::ast::SimpleAssignmentTarget::ComputedMemberExpression(m) => {
                // arr[i]++  /  obj[key]++  /  arr[i]--
                let (obj_opt, nb) = self.lower_expression(&m.object, block, _region, scope)?;
                block = nb;
                let obj_val = obj_opt.ok_or_else(|| anyhow::anyhow!("update computed member: object produced no value"))?;
                let obj_i64 = self.ensure_i64(obj_val, block)?;
                let (key_opt, nb) = self.lower_expression(&m.expression, block, _region, scope)?;
                block = nb;
                let key_val = key_opt.ok_or_else(|| anyhow::anyhow!("update computed member: key produced no value"))?;
                let key_i64 = self.ensure_i64(key_val, block)?;
                let i64t = self.i64_type();
                // Read old value.
                let old_i64: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_get_key"),
                    &[obj_i64, key_i64], &[i64t], self.loc,
                )).result(0)?.into();
                // Compute new value.
                let old_i32 = self.ensure_i32(old_i64, block)?;
                let one = self.lower_numeric_literal(1, block)?;
                let new_i32: Value<'c, 'b> = match update.operator {
                    UpdateOperator::Increment => block.append_operation(arith::addi(old_i32, one, self.loc)).result(0)?.into(),
                    UpdateOperator::Decrement => block.append_operation(arith::subi(old_i32, one, self.loc)).result(0)?.into(),
                };
                let new_i64 = self.ensure_i64(new_i32, block)?;
                // Write back.
                block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set_val_key"),
                    &[obj_i64, key_i64, new_i64], &[], self.loc,
                ));
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[key_i64], &[], self.loc));
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                if update.prefix {
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[old_i64], &[], self.loc));
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"), &[new_i64], &[], self.loc));
                    return Ok((Some(new_i64), block));
                } else {
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[new_i64], &[], self.loc));
                    return Ok((Some(old_i64), block));
                }
            }
            _ => {}
        }

        let id = match &update.argument {
            oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => id,
            _ => {
                tracing::warn!("update expression: non-identifier target not yet supported, returning undefined");
                let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                    self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
                )).result(0)?.into();
                return Ok((Some(undef), block));
            }
        };
        let name = id.name.to_string();

        // Cell variable: read/write through the single-element TsArray cell.
        if self.is_cell_var(&name) {
            let cell_ptr = *scope.get(&name).ok_or_else(|| anyhow::anyhow!("cell var undefined: {}", name))?;
            let old_actual = self.cell_read(cell_ptr, block)?; // owned (ts_arr_get retains)
            let old_i32 = self.ensure_i32(old_actual, block)?;
            let one = self.lower_numeric_literal(1, block)?;
            let new_i32: Value<'c, 'b> = match update.operator {
                UpdateOperator::Increment => block.append_operation(arith::addi(old_i32, one, self.loc)).result(0)?.into(),
                UpdateOperator::Decrement => block.append_operation(arith::subi(old_i32, one, self.loc)).result(0)?.into(),
            };
            let new_boxed = self.ensure_i64(new_i32, block)?;
            self.cell_write(cell_ptr, new_boxed, block)?;
            if update.prefix {
                // Release the old value retained by cell_read (caller gets new value).
                let old_i64 = self.ensure_i64(old_actual, block)?;
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[old_i64], &[], self.loc));
                return Ok((Some(new_boxed), block));
            } else {
                // Return old value (owned via cell_read). New value is in cell.
                return Ok((Some(old_actual), block));
            }
        }

        // ARC: Manual identifier read.
        let old_val = if let Some(&v) = scope.get(&name) {
            v
        } else {
            // Variable not in scope (e.g., inherited field from a JS-only package base class).
            tracing::warn!("update expression: variable '{}' not in scope (from JS-only package), returning undefined", name);
            let undef: Value<'c, 'b> = block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            return Ok((Some(undef), block));
        };
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


    // ── Logical assignment operators (??=, ||=, &&=) ─────────────────────

    fn lower_logical_assignment<'b>(
        &mut self,
        assign: &oxc_ast::ast::AssignmentExpression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        use oxc_ast::ast::AssignmentOperator;

        // Handle member-target logical assignment: `this.x ??= val` / `this.#x ??= val`
        match &assign.left {
            AssignmentTarget::StaticMemberExpression(m) => {
                return self.lower_logical_assignment_member(m, assign.operator, &assign.right, block, region, scope);
            }
            AssignmentTarget::PrivateFieldExpression(m) => {
                return self.lower_logical_assignment_private(m, assign.operator, &assign.right, block, region, scope);
            }
            AssignmentTarget::ComputedMemberExpression(m) => {
                // `obj[key] ??= val` / `obj[key] ||= val` / `obj[key] &&= val`
                use oxc_ast::ast::AssignmentOperator;
                let (obj_opt, nb) = self.lower_expression(&m.object, block, region, scope)?;
                block = nb;
                let obj_i64 = self.ensure_i64(obj_opt.ok_or_else(|| anyhow::anyhow!("computed ??=: no object"))?, block)?;
                let (key_opt, nb) = self.lower_expression(&m.expression, block, region, scope)?;
                block = nb;
                let key_i64 = self.ensure_i64(key_opt.ok_or_else(|| anyhow::anyhow!("computed ??=: no key"))?, block)?;
                let i64t = self.i64_type();
                let i32t = self.i32_type();
                // Current value: ts_val_get_key(obj, key)
                let current: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_get_key"),
                    &[obj_i64, key_i64], &[i64t], self.loc,
                )).result(0)?.into();
                // Check condition.
                let cond_i32: Value<'c, 'b> = match assign.operator {
                    AssignmentOperator::LogicalNullish => block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_nullish"),
                        &[current], &[i32t], self.loc,
                    )).result(0)?.into(),
                    AssignmentOperator::LogicalOr => {
                        let t = block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_truthy"),
                            &[current], &[i32t], self.loc,
                        )).result(0)?.into();
                        block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_not"),
                            &[t], &[i32t], self.loc,
                        )).result(0)?.into()
                    }
                    AssignmentOperator::LogicalAnd => block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_truthy"),
                        &[current], &[i32t], self.loc,
                    )).result(0)?.into(),
                    _ => unreachable!(),
                };
                let cond_i1 = self.ensure_i1(cond_i32, block)?;
                let merge_block = region.append_block(Block::new(&[(i64t, self.loc)]));
                let assign_block = region.append_block(Block::new(&[]));
                block.append_operation(cf::cond_br(
                    self.ctx, cond_i1, &assign_block, &merge_block, &[], &[current], self.loc,
                ));
                // assign_block: evaluate RHS, store in obj[key].
                let mut assign_scope = scope.clone();
                let (rhs_opt, rhs_end) = self.lower_expression(&assign.right, assign_block, region, &mut assign_scope)?;
                let rhs_i64 = self.ensure_i64(rhs_opt.ok_or_else(|| anyhow::anyhow!("computed ??= rhs no val"))?, rhs_end)?;
                rhs_end.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set_val_key"),
                    &[obj_i64, key_i64, rhs_i64], &[], self.loc,
                ));
                rhs_end.append_operation(cf::br(&merge_block, &[rhs_i64], self.loc));
                block = merge_block;
                let result: Value<'c, 'b> = merge_block.argument(0)?.into();
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[key_i64], &[], self.loc));
                return Ok((Some(result), block));
            }
            _ => {}
        }

        // Only identifier targets supported for the remaining path.
        let name = match &assign.left {
            AssignmentTarget::AssignmentTargetIdentifier(id) => id.name.to_string(),
            _ => bail!("logical assignment (??= / ||= / &&=) to non-identifier targets not supported yet"),
        };

        // Read LHS. For cell vars, read the actual value from the cell (not the cell pointer).
        let lhs_i64 = if self.is_cell_var(&name) {
            let cell_ptr = *scope.get(&name).ok_or_else(|| anyhow::anyhow!("undefined: {}", name))?;
            self.cell_read(cell_ptr, block)?
        } else {
            let lhs_val = *scope.get(&name).ok_or_else(|| anyhow::anyhow!("undefined: {}", name))?;
            let v = self.ensure_i64(lhs_val, block)?;
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"),
                &[v], &[], self.loc,
            ));
            v
        };

        // Compute condition i1: when true → evaluate RHS and assign; when false → keep LHS.
        let cond_i1: Value<'c, 'b> = match assign.operator {
            AssignmentOperator::LogicalNullish => {
                // ??= : assign when LHS is null/undefined
                let is_null = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_nullish"),
                    &[lhs_i64], &[self.i32_type()], self.loc,
                )).result(0)?.into();
                self.ensure_i1(is_null, block)?
            }
            AssignmentOperator::LogicalOr => {
                // ||= : assign when LHS is falsy
                let truthy = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_truthy"),
                    &[lhs_i64], &[self.i32_type()], self.loc,
                )).result(0)?.into();
                let truthy_i1 = self.ensure_i1(truthy, block)?;
                // falsy = !truthy
                let one_i1 = block.append_operation(
                    arith::constant(self.ctx, IntegerAttribute::new(self.i1_type(), 1).into(), self.loc)
                ).result(0)?.into();
                block.append_operation(arith::xori(truthy_i1, one_i1, self.loc)).result(0)?.into()
            }
            AssignmentOperator::LogicalAnd => {
                // &&= : assign when LHS is truthy
                let truthy = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_truthy"),
                    &[lhs_i64], &[self.i32_type()], self.loc,
                )).result(0)?.into();
                self.ensure_i1(truthy, block)?
            }
            _ => bail!("lower_logical_assignment called with non-logical operator"),
        };

        // Build merge block: (result: i64, ...other scope vars).
        let orig_scope = scope.clone();
        // All scope keys EXCEPT `name` — those pass through unchanged.
        let other_keys: Vec<String> = orig_scope.keys()
            .filter(|k| *k != &name)
            .cloned()
            .collect();

        let mut merge_arg_types = vec![(self.i64_type(), self.loc)];
        for k in &other_keys {
            merge_arg_types.push((orig_scope[k].r#type(), self.loc));
        }
        let merge_block = region.append_block(Block::new(&merge_arg_types));
        let rhs_eval_block = region.append_block(Block::new(&[]));

        let other_vals: Vec<Value<'c, 'b>> = other_keys.iter().map(|k| orig_scope[k]).collect();

        // cond true  → rhs_eval_block (evaluate & assign)
        // cond false → merge_block with current lhs_i64
        let mut keep_args = vec![lhs_i64];
        keep_args.extend(other_vals.iter().copied());
        block.append_operation(cf::cond_br(
            self.ctx, cond_i1,
            &rhs_eval_block, &merge_block,
            &[],         // rhs_eval_block args
            &keep_args,  // merge_block args: [lhs_i64, ...other_vals]
            self.loc,
        ));

        // rhs_eval_block: release old LHS, evaluate RHS, branch to merge.
        rhs_eval_block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[lhs_i64], &[], self.loc,
        ));
        let mut rhs_scope = orig_scope.clone();
        let (rhs_val_opt, rhs_block) = self.lower_expression(&assign.right, rhs_eval_block, region, &mut rhs_scope)?;
        let rhs_val = rhs_val_opt.ok_or_else(|| anyhow::anyhow!("logical assign: rhs produced no value"))?;
        let rhs_i64 = self.ensure_i64(rhs_val, rhs_block)?;

        let mut rhs_args = vec![rhs_i64];
        for (i, k) in other_keys.iter().enumerate() {
            let v = *rhs_scope.get(k).unwrap_or(&orig_scope[k]);
            let ty = merge_arg_types[i + 1].0;
            let coerced = self.coerce_val_to_type(v, ty, rhs_block).unwrap_or(v);
            rhs_args.push(coerced);
        }
        rhs_block.append_operation(cf::br(&merge_block, &rhs_args, self.loc));

        // After merge: result is arg 0, other scope vars from args 1..
        let result: Value<'c, 'b> = merge_block.argument(0)?.into();
        if self.is_cell_var(&name) {
            // Write new value back to cell; scope[name] stays as the cell pointer.
            let cell_ptr = *orig_scope.get(&name).unwrap();
            self.cell_write(cell_ptr, result, merge_block)?;
        } else {
            scope.insert(name, result);
        }
        for (i, k) in other_keys.iter().enumerate() {
            scope.insert(k.clone(), merge_block.argument(i + 1)?.into());
        }

        Ok((Some(result), merge_block))
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

        // Logical assignment operators use short-circuit evaluation and need special handling.
        match assign.operator {
            AssignmentOperator::LogicalNullish
            | AssignmentOperator::LogicalOr
            | AssignmentOperator::LogicalAnd => {
                return self.lower_logical_assignment(assign, block, region, scope);
            }
            _ => {}
        }

        let (rhs_opt, nb) = self.lower_expression(&assign.right, block, region, scope)?;
        block = nb;
        let rhs = rhs_opt
            .ok_or_else(|| anyhow::anyhow!("assignment: rhs produced no value"))?;

        match &assign.left {
            AssignmentTarget::AssignmentTargetIdentifier(id) => {
                let name = id.name.to_string();

                // Cell variable: read/write through the single-element TsArray cell.
                if self.is_cell_var(&name) {
                    let cell_ptr = *scope.get(&name)
                        .ok_or_else(|| anyhow::anyhow!("cell var undefined: {}", name))?;
                    let new_val: Value<'c, 'b> = match assign.operator {
                        AssignmentOperator::Assign => rhs,
                        _ => {
                            // Read actual value from cell.
                            let lhs_actual = self.cell_read(cell_ptr, block)?;
                            let lhs_i64 = self.ensure_i64(lhs_actual, block)?;
                            let rhs_i64 = self.ensure_i64(rhs, block)?;
                            let res: Value<'c, 'b> = match assign.operator {
                                AssignmentOperator::Addition => {
                                    let res = block.append_operation(func::call(
                                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_add"),
                                        &[lhs_i64, rhs_i64], &[self.i64_type()], self.loc,
                                    )).result(0)?.into();
                                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
                                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64], &[], self.loc));
                                    res
                                }
                                AssignmentOperator::Subtraction => {
                                    let lhs_i32 = self.ensure_i32(lhs_actual, block)?;
                                    let rhs_i32 = self.ensure_i32(rhs, block)?;
                                    let res_i32 = block.append_operation(arith::subi(lhs_i32, rhs_i32, self.loc)).result(0)?.into();
                                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
                                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64], &[], self.loc));
                                    res_i32
                                }
                                AssignmentOperator::Multiplication => {
                                    let lhs_i32 = self.ensure_i32(lhs_actual, block)?;
                                    let rhs_i32 = self.ensure_i32(rhs, block)?;
                                    let res_i32 = block.append_operation(arith::muli(lhs_i32, rhs_i32, self.loc)).result(0)?.into();
                                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
                                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64], &[], self.loc));
                                    res_i32
                                }
                                op => {
                                    let fn_name = match op {
                                        AssignmentOperator::Division   => "ts_div",
                                        AssignmentOperator::Remainder  => "ts_mod",
                                        AssignmentOperator::Exponential => "ts_pow",
                                        AssignmentOperator::BitwiseOR  => "ts_bitor",
                                        AssignmentOperator::BitwiseAnd => "ts_bitand",
                                        AssignmentOperator::BitwiseXOR => "ts_bitxor",
                                        AssignmentOperator::ShiftLeft  => "ts_shl",
                                        AssignmentOperator::ShiftRight => "ts_shr",
                                        AssignmentOperator::ShiftRightZeroFill => "ts_ushr",
                                        _ => bail!("unsupported compound assignment operator for cell var: {:?}", op),
                                    };
                                    let res: Value<'c, 'b> = block.append_operation(func::call(
                                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, fn_name),
                                        &[lhs_i64, rhs_i64], &[self.i64_type()], self.loc,
                                    )).result(0)?.into();
                                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
                                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64], &[], self.loc));
                                    res
                                }
                            };
                            res
                        }
                    };
                    // Write through cell. cell_write retains the new value internally.
                    self.cell_write(cell_ptr, new_val, block)?;
                    return Ok((Some(new_val), block));
                }

                // ARC: Get the new value (possibly compound).
                let new_val = match assign.operator {
                    AssignmentOperator::Assign => rhs,
                    _ => {
                        // ARC: Manual identifier read (equivalent to lower_expression(Identifier)).
                        let name = id.name.to_string();
                        let lhs = *scope.get(&name).ok_or_else(|| anyhow::anyhow!("undefined: {}", name))?;
                        let lhs_i64 = self.ensure_i64(lhs, block)?;
                        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"), &[lhs_i64], &[], self.loc));


                        let lhs_i64 = self.ensure_i64(lhs, block)?;
                        let rhs_i64 = self.ensure_i64(rhs, block)?;
                        // Use ts_add for Addition (handles strings + integers at runtime).
                        // For other operators use integer arithmetic.
                        let result_val: Value<'c, 'b> = match assign.operator {
                            AssignmentOperator::Addition => {
                                let res = block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_add"),
                                    &[lhs_i64, rhs_i64], &[self.i64_type()], self.loc,
                                )).result(0)?.into();
                                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
                                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64], &[], self.loc));
                                // Retain res so both scope and the caller (ExpressionStatement)
                                // hold an owned reference. ExpressionStatement will release
                                // the caller's ref, leaving scope's ref intact.
                                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"), &[res], &[], self.loc));
                                return {
                                    // Release old scope value, store new.
                                    let is_unowned_param = self.current_fn_params.contains(&name);
                                    if !is_unowned_param {
                                        if let Some(&old_val) = scope.get(&name) {
                                            let old_i64 = self.ensure_i64(old_val, block)?;
                                            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[old_i64], &[], self.loc));
                                        }
                                    } else {
                                        self.current_fn_params.remove(&name);
                                    }
                                    scope.insert(name, res);
                                    Ok((Some(res), block))
                                };
                            }
                            AssignmentOperator::Subtraction => {
                                let lhs_i32 = self.ensure_i32(lhs, block)?;
                                let rhs_i32 = self.ensure_i32(rhs, block)?;
                                let res_i32 = block.append_operation(arith::subi(lhs_i32, rhs_i32, self.loc)).result(0)?.into();
                                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
                                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64], &[], self.loc));
                                res_i32
                            }
                            AssignmentOperator::Multiplication => {
                                let lhs_i32 = self.ensure_i32(lhs, block)?;
                                let rhs_i32 = self.ensure_i32(rhs, block)?;
                                let res_i32 = block.append_operation(arith::muli(lhs_i32, rhs_i32, self.loc)).result(0)?.into();
                                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
                                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64], &[], self.loc));
                                res_i32
                            }
                            op => {
                                let fn_name = match op {
                                    AssignmentOperator::Division    => "ts_div",
                                    AssignmentOperator::Remainder   => "ts_mod",
                                    AssignmentOperator::Exponential => "ts_pow",
                                    AssignmentOperator::BitwiseOR   => "ts_bitor",
                                    AssignmentOperator::BitwiseAnd  => "ts_bitand",
                                    AssignmentOperator::BitwiseXOR  => "ts_bitxor",
                                    AssignmentOperator::ShiftLeft   => "ts_shl",
                                    AssignmentOperator::ShiftRight  => "ts_shr",
                                    AssignmentOperator::ShiftRightZeroFill => "ts_ushr",
                                    _ => bail!("unsupported compound assignment operator: {:?}", op),
                                };
                                let res: Value<'c, 'b> = block.append_operation(func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, fn_name),
                                    &[lhs_i64, rhs_i64], &[self.i64_type()], self.loc,
                                )).result(0)?.into();
                                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
                                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64], &[], self.loc));
                                res
                            }
                        };

                        result_val
                    }
                };

                // ARC: Release the old value in the scope — unless it is still an
                // unowned function parameter (params are "borrowed" refs from the
                // caller; the caller's post-call ts_release_val owns them).
                // Once a param is first assigned, it is promoted to a local (owned).
                let is_unowned_param = self.current_fn_params.contains(&name);
                if !is_unowned_param {
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
                } else {
                    // Promote: param is now an owned local variable.
                    self.current_fn_params.remove(&name);
                }

                scope.insert(name.clone(), new_val);

                // If this variable is a captured env var, write the mutation back to the env array.
                if let Some(&env_idx) = self.closure_env_indices.get(&name) {
                    if let Some(&env_arr) = scope.get("__env") {
                        let new_i64_wb = self.ensure_i64(new_val, block)?;
                        let env_i64 = self.ensure_i64(env_arr, block)?;
                        let idx_val = block.append_operation(arith::constant(
                            self.ctx,
                            IntegerAttribute::new(self.i32_type(), env_idx as i64).into(),
                            self.loc,
                        )).result(0)?.into();
                        block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_set"),
                            &[env_i64, idx_val, new_i64_wb], &[], self.loc,
                        ));
                    }
                }

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
            AssignmentTarget::PrivateFieldExpression(priv_member) => {
                self.lower_private_field_assignment(&**priv_member, assign.operator, rhs, block, region, scope)
            }
            AssignmentTarget::ArrayAssignmentTarget(arr_target) => {
                use oxc_ast::ast::{AssignmentTargetMaybeDefault, AssignmentTarget as AT};
                // Evaluate rhs once, then assign elements to each target slot.
                // rhs is already evaluated.
                let rhs_i64 = self.ensure_i64(rhs, block)?;
                let i32_type = self.i32_type();
                let i64_type = self.i64_type();
                for (i, elem) in arr_target.elements.iter().enumerate() {
                    let Some(maybe_default) = elem else { continue };
                    // Get the binding target (skip default for now — just use the binding)
                    let binding_target = match maybe_default {
                        AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(wtd) => &wtd.binding,
                        other => {
                            // AssignmentTargetMaybeDefault inherits from AssignmentTarget
                            // via @inherit. We need to access it as AssignmentTarget.
                            // Unfortunately there's no clean cast; extract Identifier variant.
                            match other {
                                AssignmentTargetMaybeDefault::AssignmentTargetIdentifier(id_ref) => {
                                    let name = id_ref.name.to_string();
                                    let idx_val: Value<'c, 'b> = block.append_operation(
                                        melior::dialect::arith::constant(self.ctx,
                                            melior::ir::attribute::IntegerAttribute::new(i32_type, i as i64).into(), self.loc)
                                    ).result(0)?.into();
                                    let elem_val: Value<'c, 'b> = block.append_operation(
                                        melior::dialect::func::call(self.ctx,
                                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
                                            &[rhs_i64, idx_val], &[i64_type], self.loc)
                                    ).result(0)?.into();
                                    if self.is_cell_var(&name) {
                                        let cell_ptr = *scope.get(&name).ok_or_else(|| anyhow::anyhow!("undefined cell: {name}"))?;
                                        self.cell_write(cell_ptr, elem_val, block)?;
                                    } else {
                                        let old = scope.insert(name.clone(), elem_val);
                                        if let Some(old_val) = old {
                                            let old_i64 = self.ensure_i64(old_val, block)?;
                                            block.append_operation(melior::dialect::func::call(self.ctx,
                                                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                                &[old_i64], &[], self.loc));
                                        }
                                    }
                                    continue;
                                }
                                AssignmentTargetMaybeDefault::StaticMemberExpression(m) => {
                                    // e.g. [arr[i], arr[j]] swap pattern
                                    let idx_val: Value<'c, 'b> = block.append_operation(
                                        melior::dialect::arith::constant(self.ctx,
                                            melior::ir::attribute::IntegerAttribute::new(i32_type, i as i64).into(), self.loc)
                                    ).result(0)?.into();
                                    let elem_val: Value<'c, 'b> = block.append_operation(
                                        melior::dialect::func::call(self.ctx,
                                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
                                            &[rhs_i64, idx_val], &[i64_type], self.loc)
                                    ).result(0)?.into();
                                    self.lower_static_member_assignment(&**m, oxc_ast::ast::AssignmentOperator::Assign, elem_val, block, region, scope)?;
                                    continue;
                                }
                                AssignmentTargetMaybeDefault::ComputedMemberExpression(m) => {
                                    let idx_val: Value<'c, 'b> = block.append_operation(
                                        melior::dialect::arith::constant(self.ctx,
                                            melior::ir::attribute::IntegerAttribute::new(i32_type, i as i64).into(), self.loc)
                                    ).result(0)?.into();
                                    let elem_val: Value<'c, 'b> = block.append_operation(
                                        melior::dialect::func::call(self.ctx,
                                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
                                            &[rhs_i64, idx_val], &[i64_type], self.loc)
                                    ).result(0)?.into();
                                    self.lower_computed_member_assignment(&**m, oxc_ast::ast::AssignmentOperator::Assign, elem_val, block, region, scope)?;
                                    continue;
                                }
                                _ => bail!("unsupported array assignment target element: {:?}", maybe_default),
                            }
                        }
                    };
                    // AssignmentTargetWithDefault: get element then apply default if undefined
                    let wtd = match maybe_default {
                        AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(w) => w,
                        _ => unreachable!(),
                    };
                    let idx_val: Value<'c, 'b> = block.append_operation(
                        melior::dialect::arith::constant(self.ctx,
                            melior::ir::attribute::IntegerAttribute::new(i32_type, i as i64).into(), self.loc)
                    ).result(0)?.into();
                    let elem_val: Value<'c, 'b> = block.append_operation(
                        melior::dialect::func::call(self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
                            &[rhs_i64, idx_val], &[i64_type], self.loc)
                    ).result(0)?.into();
                    // Apply default if undefined
                    let is_undef: Value<'c, 'b> = block.append_operation(melior::dialect::func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_undefined"),
                        &[elem_val], &[self.i32_type()], self.loc,
                    )).result(0)?.into();
                    let is_undef_i1 = self.ensure_i1(is_undef, block)?;
                    let default_block = region.append_block(melior::ir::Block::new(&[]));
                    let use_elem_block = region.append_block(melior::ir::Block::new(&[]));
                    let merge_block = region.append_block(melior::ir::Block::new(&[(i64_type, self.loc)]));
                    block.append_operation(melior::dialect::cf::cond_br(self.ctx, is_undef_i1, &default_block, &use_elem_block, &[], &[], self.loc));
                    // default branch
                    let (def_opt, nb) = self.lower_expression(&wtd.init, default_block, region, scope)?;
                    let def_block = nb;
                    let def_val = self.ensure_i64(def_opt.unwrap_or(elem_val), def_block)?;
                    def_block.append_operation(melior::dialect::cf::br(&merge_block, &[def_val], self.loc));
                    // use elem branch
                    let elem_i64 = self.ensure_i64(elem_val, use_elem_block)?;
                    use_elem_block.append_operation(melior::dialect::cf::br(&merge_block, &[elem_i64], self.loc));
                    // merge
                    let final_val: Value<'c, 'b> = merge_block.argument(0)?.into();
                    block = merge_block;
                    // Assign final_val to the binding_target
                    if let AT::AssignmentTargetIdentifier(id_ref) = binding_target {
                        let name = id_ref.name.to_string();
                        if self.is_cell_var(&name) {
                            let cell_ptr = *scope.get(&name).ok_or_else(|| anyhow::anyhow!("undefined cell: {name}"))?;
                            self.cell_write(cell_ptr, final_val, block)?;
                        } else {
                            let old = scope.insert(name.clone(), final_val);
                            if let Some(old_val) = old {
                                let old_i64 = self.ensure_i64(old_val, block)?;
                                block.append_operation(melior::dialect::func::call(self.ctx,
                                    FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                    &[old_i64], &[], self.loc));
                            }
                        }
                    } else {
                        bail!("unsupported array assignment target binding: {:?}", binding_target);
                    }
                }
                // Handle rest element if present
                if let Some(rest) = &arr_target.rest {
                    let n_assigned = arr_target.elements.len();
                    let n_val: Value<'c, 'b> = block.append_operation(
                        melior::dialect::arith::constant(self.ctx,
                            melior::ir::attribute::IntegerAttribute::new(i32_type, n_assigned as i64).into(), self.loc)
                    ).result(0)?.into();
                    let rest_arr: Value<'c, 'b> = block.append_operation(
                        melior::dialect::func::call(self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "ts_arr_rest"),
                            &[rhs_i64, n_val], &[i64_type], self.loc)
                    ).result(0)?.into();
                    if let AT::AssignmentTargetIdentifier(id_ref) = &rest.target {
                        let name = id_ref.name.to_string();
                        let old = scope.insert(name.clone(), rest_arr);
                        if let Some(old_val) = old {
                            let old_i64 = self.ensure_i64(old_val, block)?;
                            block.append_operation(melior::dialect::func::call(self.ctx,
                                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                &[old_i64], &[], self.loc));
                        }
                    }
                }
                // rhs_i64 is returned as the value of the assignment expression;
                // the caller (ExpressionStatement handler) will release it.
                Ok((Some(rhs_i64), block))
            }
            AssignmentTarget::ObjectAssignmentTarget(obj_target) => {
                use oxc_ast::ast::AssignmentTargetProperty;
                let rhs_i64 = self.ensure_i64(rhs, block)?;
                let i64_type = self.i64_type();
                for prop in &obj_target.properties {
                    match prop {
                        AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id_prop) => {
                            let name = id_prop.binding.name.to_string();
                            let key_ptr = self.get_string_ptr(&name, block)?;
                            let val: Value<'c, 'b> = block.append_operation(melior::dialect::func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                                &[rhs_i64, key_ptr], &[i64_type], self.loc,
                            )).result(0)?.into();
                            // Apply default if needed
                            let final_val = if let Some(default_expr) = &id_prop.init {
                                let is_undef: Value<'c, 'b> = block.append_operation(melior::dialect::func::call(
                                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_undefined"),
                                    &[val], &[self.i32_type()], self.loc,
                                )).result(0)?.into();
                                let is_undef_i1 = self.ensure_i1(is_undef, block)?;
                                let def_block = region.append_block(melior::ir::Block::new(&[]));
                                let use_block = region.append_block(melior::ir::Block::new(&[]));
                                let merge = region.append_block(melior::ir::Block::new(&[(i64_type, self.loc)]));
                                block.append_operation(melior::dialect::cf::cond_br(self.ctx, is_undef_i1, &def_block, &use_block, &[], &[], self.loc));
                                let (def_opt, nb) = self.lower_expression(default_expr, def_block, region, scope)?;
                                let db = nb;
                                let def_i64 = self.ensure_i64(def_opt.unwrap_or(val), db)?;
                                db.append_operation(melior::dialect::cf::br(&merge, &[def_i64], self.loc));
                                let use_i64 = self.ensure_i64(val, use_block)?;
                                use_block.append_operation(melior::dialect::cf::br(&merge, &[use_i64], self.loc));
                                let f: Value<'c, 'b> = merge.argument(0)?.into();
                                block = merge;
                                f
                            } else { val };
                            if self.is_cell_var(&name) {
                                let cell_ptr = *scope.get(&name).ok_or_else(|| anyhow::anyhow!("undefined cell: {name}"))?;
                                self.cell_write(cell_ptr, final_val, block)?;
                            } else {
                                let old = scope.insert(name.clone(), final_val);
                                if let Some(ov) = old {
                                    let ov_i64 = self.ensure_i64(ov, block)?;
                                    block.append_operation(melior::dialect::func::call(self.ctx,
                                        FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                        &[ov_i64], &[], self.loc));
                                }
                            }
                        }
                        AssignmentTargetProperty::AssignmentTargetPropertyProperty(kv) => {
                            use oxc_ast::ast::{PropertyKey, AssignmentTargetMaybeDefault as ATMD};
                            let key_str = match &kv.name {
                                PropertyKey::StaticIdentifier(id) => id.name.to_string(),
                                PropertyKey::StringLiteral(s) => s.value.to_string(),
                                _ => bail!("unsupported object assignment target key: {:?}", kv.name),
                            };
                            let key_ptr = self.get_string_ptr(&key_str, block)?;
                            let val: Value<'c, 'b> = block.append_operation(melior::dialect::func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                                &[rhs_i64, key_ptr], &[i64_type], self.loc,
                            )).result(0)?.into();
                            // Assign to binding target
                            match &kv.binding {
                                ATMD::AssignmentTargetIdentifier(id_ref) => {
                                    let name = id_ref.name.to_string();
                                    if self.is_cell_var(&name) {
                                        let cell_ptr = *scope.get(&name).ok_or_else(|| anyhow::anyhow!("undefined cell: {name}"))?;
                                        self.cell_write(cell_ptr, val, block)?;
                                    } else {
                                        let old = scope.insert(name.clone(), val);
                                        if let Some(ov) = old {
                                            let ov_i64 = self.ensure_i64(ov, block)?;
                                            block.append_operation(melior::dialect::func::call(self.ctx,
                                                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                                &[ov_i64], &[], self.loc));
                                        }
                                    }
                                }
                                _ => bail!("unsupported object assignment target binding: {:?}", kv.binding),
                            }
                        }
                    }
                }
                // Handle rest: { ...rest } = obj
                if let Some(rest) = &obj_target.rest {
                    use oxc_ast::ast::AssignmentTarget as AT2;
                    // Collect key list
                    let mut keys_arr: Value<'c, 'b> = block.append_operation(melior::dialect::func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
                        &[block.append_operation(melior::dialect::arith::constant(self.ctx,
                            melior::ir::attribute::IntegerAttribute::new(self.i32_type(), 0).into(), self.loc)).result(0)?.into()],
                        &[i64_type], self.loc,
                    )).result(0)?.into();
                    for prop in &obj_target.properties {
                        if let AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id_prop) = prop {
                            let k_str = self.lower_string_literal(&id_prop.binding.name.to_string(), block)?;
                            let push_res: Value<'c, 'b> = block.append_operation(melior::dialect::func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_push"),
                                &[keys_arr, k_str], &[i64_type], self.loc,
                            )).result(0)?.into();
                            block.append_operation(melior::dialect::func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[push_res], &[], self.loc));
                            block.append_operation(melior::dialect::func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[k_str], &[], self.loc));
                        }
                    }
                    let rest_obj: Value<'c, 'b> = block.append_operation(melior::dialect::func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_rest"),
                        &[rhs_i64, keys_arr], &[i64_type], self.loc,
                    )).result(0)?.into();
                    block.append_operation(melior::dialect::func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[keys_arr], &[], self.loc));
                    if let AT2::AssignmentTargetIdentifier(id_ref) = &rest.target {
                        let name = id_ref.name.to_string();
                        let old = scope.insert(name.clone(), rest_obj);
                        if let Some(ov) = old {
                            let ov_i64 = self.ensure_i64(ov, block)?;
                            block.append_operation(melior::dialect::func::call(self.ctx,
                                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                &[ov_i64], &[], self.loc));
                        }
                    }
                }
                // rhs_i64 is returned as the value of the assignment expression;
                // the caller (ExpressionStatement handler) will release it.
                Ok((Some(rhs_i64), block))
            }
            // TypeScript type assertions as assignment targets: strip the cast, recurse.
            AssignmentTarget::TSAsExpression(ts_as) => {
                // (expr as T) = rhs — the cast doesn't affect runtime assignment.
                // Wrap in a synthetic assignment using the inner expression as target.
                use oxc_ast::ast::Expression;
                match &ts_as.expression {
                    Expression::Identifier(id) => {
                        let name = id.name.to_string();
                        // Treat as a simple identifier assignment.
                        let new_val = rhs;
                        if self.is_cell_var(&name) {
                            let cell_ptr = *scope.get(&name).ok_or_else(|| anyhow::anyhow!("undefined cell: {name}"))?;
                            self.cell_write(cell_ptr, new_val, block)?;
                        } else {
                            if let Some(old) = scope.insert(name, new_val) {
                                let old_i64 = self.ensure_i64(old, block)?;
                                block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[old_i64], &[], self.loc));
                            }
                        }
                        Ok((Some(new_val), block))
                    }
                    _ => {
                        tracing::warn!("TSAsExpression assignment with non-identifier inner expression, skipping");
                        Ok((Some(rhs), block))
                    }
                }
            }
            AssignmentTarget::TSSatisfiesExpression(ts_sat) => {
                use oxc_ast::ast::Expression;
                match &ts_sat.expression {
                    Expression::Identifier(id) => {
                        let name = id.name.to_string();
                        let new_val = rhs;
                        if let Some(old) = scope.insert(name, new_val) {
                            let old_i64 = self.ensure_i64(old, block)?;
                            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[old_i64], &[], self.loc));
                        }
                        Ok((Some(new_val), block))
                    }
                    _ => { tracing::warn!("TSSatisfiesExpression assignment: skipping"); Ok((Some(rhs), block)) }
                }
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
        // Static class field assignment: ClassName.field = value → ts_set_module_global
        if operator == AssignmentOperator::Assign {
            if let Expression::Identifier(id) = &member.object {
                let class_name = id.name.as_str();
                let field_name = member.property.name.as_str();
                let is_static_field = self.classes.get(class_name)
                    .map(|sig| sig.static_fields.contains(field_name))
                    .unwrap_or(false);
                if is_static_field {
                    let global_key = format!("__static_{}_{}", class_name, field_name);
                    let key_ptr = self.get_string_ptr(&global_key, block)?;
                    let val_i64 = self.ensure_i64(rhs, block)?;
                    block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_set_module_global"),
                        &[key_ptr, val_i64], &[], self.loc,
                    ));
                    // ts_set_module_global retains; return rhs with its original ownership.
                    return Ok((Some(rhs), block));
                }
            }
        }
        // Handle compound assignment operators to static member expressions.
        if operator != AssignmentOperator::Assign {
            // For compound assignments (+=, -=, ||=, etc.), read the current value first.
            let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
            block = nb;
            let obj = match obj_opt {
                Some(v) => v,
                None => {
                    let u: Value<'c,'b> = block.append_operation(arith::constant(self.ctx, IntegerAttribute::new(self.i64_type(), 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc)).result(0)?.into();
                    u
                }
            };
            let obj_i64 = self.ensure_i64(obj, block)?;
            let prop_name = member.property.name.to_string();
            let key_ptr = self.get_string_ptr(&prop_name, block)?;
            let i64t = self.i64_type();

            // For logical assignment operators, short-circuit if condition is met.
            if matches!(operator, AssignmentOperator::LogicalOr | AssignmentOperator::LogicalAnd | AssignmentOperator::LogicalNullish) {
                let lhs_i64: Value<'c, 'b> = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                    &[obj_i64, key_ptr], &[i64t], self.loc,
                )).result(0)?.into();
                // Evaluate condition
                let cond_i32: Value<'c, 'b> = match operator {
                    AssignmentOperator::LogicalOr => block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_truthy"),
                        &[lhs_i64], &[self.i32_type()], self.loc,
                    )).result(0)?.into(),
                    AssignmentOperator::LogicalAnd => {
                        let t = block.append_operation(func::call(
                            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_truthy"),
                            &[lhs_i64], &[self.i32_type()], self.loc,
                        )).result(0)?.into();
                        // &&= only assigns if LHS is truthy (opposite of ||=)
                        let zero = block.append_operation(arith::constant(self.ctx, IntegerAttribute::new(self.i32_type(), 0).into(), self.loc)).result(0)?.into();
                        block.append_operation(arith::cmpi(self.ctx, arith::CmpiPredicate::Eq, t, zero, self.loc)).result(0)?.into()
                    }
                    _ => block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_nullish"),
                        &[lhs_i64], &[self.i32_type()], self.loc,
                    )).result(0)?.into(),
                };
                let cond_i1 = self.ensure_i1(cond_i32, block)?;
                let merge_block = region.append_block(Block::new(&[(i64t, self.loc)]));
                let assign_block = region.append_block(Block::new(&[]));
                let skip_block = region.append_block(Block::new(&[]));
                // skip: keep lhs unchanged
                block.append_operation(cf::cond_br(self.ctx, cond_i1, &skip_block, &assign_block, &[], &[], self.loc));
                skip_block.append_operation(cf::br(&merge_block, &[lhs_i64], self.loc));
                // assign: write rhs
                let rhs_i64 = self.ensure_i64(rhs, assign_block)?;
                assign_block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
                    &[obj_i64, key_ptr, rhs_i64], &[], self.loc,
                ));
                assign_block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
                assign_block.append_operation(cf::br(&merge_block, &[rhs_i64], self.loc));
                let result: Value<'c, 'b> = merge_block.argument(0)?.into();
                merge_block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                return Ok((Some(result), merge_block));
            }

            // Arithmetic/bitwise compound: read + op + write
            let lhs_i64: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                &[obj_i64, key_ptr], &[i64t], self.loc,
            )).result(0)?.into();
            let rhs_i64 = self.ensure_i64(rhs, block)?;
            let result_i64: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, match operator {
                    AssignmentOperator::Addition => "ts_add",
                    AssignmentOperator::Subtraction => "ts_sub",
                    AssignmentOperator::Multiplication => "ts_mul",
                    AssignmentOperator::Division => "ts_div",
                    AssignmentOperator::Remainder => "ts_mod",
                    AssignmentOperator::Exponential => "ts_pow",
                    AssignmentOperator::BitwiseOR => "ts_bitor",
                    AssignmentOperator::BitwiseAnd => "ts_bitand",
                    AssignmentOperator::BitwiseXOR => "ts_bitxor",
                    AssignmentOperator::ShiftLeft => "ts_shl",
                    AssignmentOperator::ShiftRight => "ts_shr",
                    AssignmentOperator::ShiftRightZeroFill => "ts_ushr",
                    _ => "ts_add", // fallback
                }),
                &[lhs_i64, rhs_i64], &[i64t], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
                &[obj_i64, key_ptr, result_i64], &[], self.loc,
            ));
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
            return Ok((Some(result_i64), block));
        }
        let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
        block = nb;
        let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("member assignment: object produced no value"))?;
        let obj_i64 = self.ensure_i64(obj, block)?;

        // Check for setter dispatch
        let prop_name = member.property.name.to_string();
        let setter_mangled: Option<String> = if let Expression::Identifier(id) = &member.object {
            self.var_class_types.get(id.name.as_str())
                .cloned()
                .and_then(|cn| {
                    self.classes.get(&cn).and_then(|sig| {
                        if sig.setters.contains(&prop_name) {
                            Some(format!("__class_{}_set_{}", cn, prop_name))
                        } else {
                            None
                        }
                    })
                })
        } else {
            None
        };

        if let Some(setter_name) = setter_mangled {
            let val_i64 = self.ensure_i64(rhs, block)?;
            block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, &setter_name),
                &[obj_i64, val_i64],
                &[self.i64_type()],
                self.loc,
            ));
            block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[obj_i64], &[], self.loc,
            ));
            return Ok((Some(rhs), block));
        }

        let key_ptr = self.get_string_ptr(&prop_name, block)?;
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
        let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
        block = nb;
        let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("computed assignment: object produced no value"))?;
        let obj_i64 = self.ensure_i64(obj, block)?;

        let (idx_opt, nb) = self.lower_expression(&member.expression, block, region, scope)?;
        block = nb;
        let idx = idx_opt.ok_or_else(|| anyhow::anyhow!("computed assignment: index produced no value"))?;
        let idx_i64 = self.ensure_i64(idx, block)?;

        let new_val: Value<'c, 'b> = if operator == AssignmentOperator::Assign {
            self.ensure_i64(rhs, block)?
        } else {
            // Read current value
            let cur: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_val_get_key"),
                &[obj_i64, idx_i64], &[self.i64_type()], self.loc,
            )).result(0)?.into();
            let lhs_i64 = self.ensure_i64(cur, block)?;
            let rhs_i64 = self.ensure_i64(rhs, block)?;
            let fn_name = match operator {
                AssignmentOperator::Addition       => "ts_add",
                AssignmentOperator::Subtraction    => "ts_sub",
                AssignmentOperator::Multiplication => "ts_mul",
                AssignmentOperator::Division       => "ts_div",
                AssignmentOperator::Remainder      => "ts_mod",
                AssignmentOperator::Exponential    => "ts_pow",
                AssignmentOperator::BitwiseOR      => "ts_bitor",
                AssignmentOperator::BitwiseAnd     => "ts_bitand",
                AssignmentOperator::BitwiseXOR     => "ts_bitxor",
                AssignmentOperator::ShiftLeft      => "ts_shl",
                AssignmentOperator::ShiftRight     => "ts_shr",
                AssignmentOperator::ShiftRightZeroFill => "ts_ushr",
                op => bail!("unsupported compound assignment operator on computed member: {:?}", op),
            };
            let res: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, fn_name),
                &[lhs_i64, rhs_i64], &[self.i64_type()], self.loc,
            )).result(0)?.into();
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64], &[], self.loc));
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64], &[], self.loc));
            res
        };

        // Use ts_obj_set_val_key for dynamic key access (works for both string and integer keys).
        block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set_val_key"),
            &[obj_i64, idx_i64, new_val],
            &[],
            self.loc,
        ));

        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[idx_i64], &[], self.loc));

        Ok((Some(new_val), block))
    }

    fn lower_private_field_assignment<'b>(
        &mut self,
        member: &oxc_ast::ast::PrivateFieldExpression<'_>,
        operator: oxc_ast::ast::AssignmentOperator,
        rhs: Value<'c, 'b>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        use oxc_ast::ast::AssignmentOperator;
        let field_key = format!("__priv_{}", member.field.name.as_str());
        let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
        block = nb;
        let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("private field assignment: object produced no value"))?;
        let obj_i64 = self.ensure_i64(obj, block)?;
        let key_ptr = self.get_string_ptr(&field_key, block)?;
        let new_val: Value<'c, 'b> = if operator == AssignmentOperator::Assign {
            self.ensure_i64(rhs, block)?
        } else {
            // Compound assignment: read current value, compute new value.
            let i64t = self.i64_type();
            let cur: Value<'c, 'b> = block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                &[obj_i64, key_ptr], &[i64t], self.loc,
            )).result(0)?.into();
            let lhs_i32 = self.ensure_i32(cur, block)?;
            let rhs_i32 = self.ensure_i32(rhs, block)?;
            let res_i32: Value<'c, 'b> = match operator {
                AssignmentOperator::Addition       => block.append_operation(arith::addi(lhs_i32, rhs_i32, self.loc)).result(0)?.into(),
                AssignmentOperator::Subtraction    => block.append_operation(arith::subi(lhs_i32, rhs_i32, self.loc)).result(0)?.into(),
                AssignmentOperator::Multiplication => block.append_operation(arith::muli(lhs_i32, rhs_i32, self.loc)).result(0)?.into(),
                AssignmentOperator::Division       => block.append_operation(arith::divsi(lhs_i32, rhs_i32, self.loc)).result(0)?.into(),
                AssignmentOperator::Remainder      => block.append_operation(arith::remsi(lhs_i32, rhs_i32, self.loc)).result(0)?.into(),
                AssignmentOperator::BitwiseAnd     => block.append_operation(arith::andi(lhs_i32, rhs_i32, self.loc)).result(0)?.into(),
                AssignmentOperator::BitwiseOR      => block.append_operation(arith::ori(lhs_i32, rhs_i32, self.loc)).result(0)?.into(),
                AssignmentOperator::BitwiseXOR     => block.append_operation(arith::xori(lhs_i32, rhs_i32, self.loc)).result(0)?.into(),
                AssignmentOperator::ShiftLeft      => block.append_operation(arith::shli(lhs_i32, rhs_i32, self.loc)).result(0)?.into(),
                AssignmentOperator::ShiftRight     => block.append_operation(arith::shrsi(lhs_i32, rhs_i32, self.loc)).result(0)?.into(),
                AssignmentOperator::ShiftRightZeroFill => block.append_operation(arith::shrui(lhs_i32, rhs_i32, self.loc)).result(0)?.into(),
                AssignmentOperator::Exponential => {
                    let lhs_i64_2 = self.ensure_i64(cur, block)?;
                    let rhs_i64_2 = self.ensure_i64(rhs, block)?;
                    let res: Value<'c, 'b> = block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_pow"),
                        &[lhs_i64_2, rhs_i64_2], &[self.i64_type()], self.loc,
                    )).result(0)?.into();
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[lhs_i64_2], &[], self.loc));
                    block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[rhs_i64_2], &[], self.loc));
                    // Return early since res is already i64
                    return {
                        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"), &[obj_i64, key_ptr, res], &[], self.loc));
                        block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[obj_i64], &[], self.loc));
                        Ok((Some(res), block))
                    };
                }
                op => bail!("unsupported compound assignment operator on private field: {:?}", op),
            };
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[cur], &[], self.loc));
            block.append_operation(func::call(self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"), &[self.ensure_i64(rhs, block)?], &[], self.loc));
            self.ensure_i64(res_i32, block)?
        };
        block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
            &[obj_i64, key_ptr, new_val],
            &[], self.loc,
        ));
        block.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[obj_i64], &[], self.loc,
        ));
        Ok((Some(new_val), block))
    }

    /// Logical assignment to a static member: `obj.prop ??= rhs` / `obj.prop ||= rhs` / `obj.prop &&= rhs`
    fn lower_logical_assignment_member<'b>(
        &mut self,
        member: &oxc_ast::ast::StaticMemberExpression<'_>,
        operator: oxc_ast::ast::AssignmentOperator,
        rhs_expr: &Expression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        use oxc_ast::ast::AssignmentOperator;

        let prop_name = member.property.name.to_string();

        // Read current value of lhs
        let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
        block = nb;
        let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("logical assign member: object produced no value"))?;
        let obj_i64 = self.ensure_i64(obj, block)?;
        // Retain obj since we may use it again in the write path
        block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"),
            &[obj_i64], &[], self.loc,
        ));

        let key_ptr = self.get_string_ptr(&prop_name, block)?;
        let lhs_i64: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
            &[obj_i64, key_ptr], &[self.i64_type()], self.loc,
        )).result(0)?.into();

        // Compute condition i1
        let cond_i1: Value<'c, 'b> = match operator {
            AssignmentOperator::LogicalNullish => {
                let is_null = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_nullish"),
                    &[lhs_i64], &[self.i32_type()], self.loc,
                )).result(0)?.into();
                self.ensure_i1(is_null, block)?
            }
            AssignmentOperator::LogicalOr => {
                let truthy = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_truthy"),
                    &[lhs_i64], &[self.i32_type()], self.loc,
                )).result(0)?.into();
                let truthy_i1 = self.ensure_i1(truthy, block)?;
                let one_i1 = block.append_operation(
                    arith::constant(self.ctx, IntegerAttribute::new(self.i1_type(), 1).into(), self.loc)
                ).result(0)?.into();
                block.append_operation(arith::xori(truthy_i1, one_i1, self.loc)).result(0)?.into()
            }
            AssignmentOperator::LogicalAnd => {
                let truthy = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_truthy"),
                    &[lhs_i64], &[self.i32_type()], self.loc,
                )).result(0)?.into();
                self.ensure_i1(truthy, block)?
            }
            _ => bail!("lower_logical_assignment_member: non-logical operator"),
        };

        let merge_block = region.append_block(Block::new(&[(self.i64_type(), self.loc)]));
        let rhs_eval_block = region.append_block(Block::new(&[]));

        // cond true → evaluate rhs and assign; cond false → keep lhs
        block.append_operation(cf::cond_br(
            self.ctx, cond_i1,
            &rhs_eval_block, &merge_block,
            &[], &[lhs_i64],
            self.loc,
        ));

        // rhs_eval_block: release lhs, evaluate rhs, assign, branch
        rhs_eval_block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[lhs_i64], &[], self.loc,
        ));
        let mut rhs_scope = scope.clone();
        let (rhs_val_opt, rhs_block) = self.lower_expression(rhs_expr, rhs_eval_block, region, &mut rhs_scope)?;
        let rhs_val = rhs_val_opt.ok_or_else(|| anyhow::anyhow!("logical assign member: rhs produced no value"))?;
        let rhs_i64 = self.ensure_i64(rhs_val, rhs_block)?;
        // Assign to obj.prop
        let key_ptr2 = self.get_string_ptr(&prop_name, rhs_block)?;
        rhs_block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
            &[obj_i64, key_ptr2, rhs_i64], &[], self.loc,
        ));
        rhs_block.append_operation(cf::br(&merge_block, &[rhs_i64], self.loc));

        let result: Value<'c, 'b> = merge_block.argument(0)?.into();
        // Release the object retain we took at the start
        merge_block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[obj_i64], &[], self.loc,
        ));

        Ok((Some(result), merge_block))
    }

    /// Logical assignment to a private field: `this.#field ??= rhs` etc.
    fn lower_logical_assignment_private<'b>(
        &mut self,
        member: &oxc_ast::ast::PrivateFieldExpression<'_>,
        operator: oxc_ast::ast::AssignmentOperator,
        rhs_expr: &Expression<'_>,
        mut block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        use oxc_ast::ast::AssignmentOperator;

        let field_key = format!("__priv_{}", member.field.name.as_str());

        // Read current value of lhs
        let (obj_opt, nb) = self.lower_expression(&member.object, block, region, scope)?;
        block = nb;
        let obj = obj_opt.ok_or_else(|| anyhow::anyhow!("logical assign private: object produced no value"))?;
        let obj_i64 = self.ensure_i64(obj, block)?;
        // Retain obj since we may use it again in the write path
        block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_retain_val"),
            &[obj_i64], &[], self.loc,
        ));

        let key_ptr = self.get_string_ptr(&field_key, block)?;
        let lhs_i64: Value<'c, 'b> = block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
            &[obj_i64, key_ptr], &[self.i64_type()], self.loc,
        )).result(0)?.into();

        // Compute condition i1
        let cond_i1: Value<'c, 'b> = match operator {
            AssignmentOperator::LogicalNullish => {
                let is_null = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_nullish"),
                    &[lhs_i64], &[self.i32_type()], self.loc,
                )).result(0)?.into();
                self.ensure_i1(is_null, block)?
            }
            AssignmentOperator::LogicalOr => {
                let truthy = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_truthy"),
                    &[lhs_i64], &[self.i32_type()], self.loc,
                )).result(0)?.into();
                let truthy_i1 = self.ensure_i1(truthy, block)?;
                let one_i1 = block.append_operation(
                    arith::constant(self.ctx, IntegerAttribute::new(self.i1_type(), 1).into(), self.loc)
                ).result(0)?.into();
                block.append_operation(arith::xori(truthy_i1, one_i1, self.loc)).result(0)?.into()
            }
            AssignmentOperator::LogicalAnd => {
                let truthy = block.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_truthy"),
                    &[lhs_i64], &[self.i32_type()], self.loc,
                )).result(0)?.into();
                self.ensure_i1(truthy, block)?
            }
            _ => bail!("lower_logical_assignment_private: non-logical operator"),
        };

        let merge_block = region.append_block(Block::new(&[(self.i64_type(), self.loc)]));
        let rhs_eval_block = region.append_block(Block::new(&[]));

        // cond true → evaluate rhs and assign; cond false → keep lhs
        block.append_operation(cf::cond_br(
            self.ctx, cond_i1,
            &rhs_eval_block, &merge_block,
            &[], &[lhs_i64],
            self.loc,
        ));

        // rhs_eval_block: release lhs, evaluate rhs, assign, branch
        rhs_eval_block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[lhs_i64], &[], self.loc,
        ));
        let mut rhs_scope = scope.clone();
        let (rhs_val_opt, rhs_block) = self.lower_expression(rhs_expr, rhs_eval_block, region, &mut rhs_scope)?;
        let rhs_val = rhs_val_opt.ok_or_else(|| anyhow::anyhow!("logical assign private: rhs produced no value"))?;
        let rhs_i64 = self.ensure_i64(rhs_val, rhs_block)?;
        let key_ptr2 = self.get_string_ptr(&field_key, rhs_block)?;
        rhs_block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
            &[obj_i64, key_ptr2, rhs_i64], &[], self.loc,
        ));
        rhs_block.append_operation(cf::br(&merge_block, &[rhs_i64], self.loc));

        let result: Value<'c, 'b> = merge_block.argument(0)?.into();
        // Release the object retain we took at the start
        merge_block.append_operation(func::call(
            self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[obj_i64], &[], self.loc,
        ));

        Ok((Some(result), merge_block))
    }
}

