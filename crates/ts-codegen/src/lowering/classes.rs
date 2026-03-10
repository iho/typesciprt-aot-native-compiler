use super::*;

impl<'c, 'm> Lowerer<'c, 'm> {
    // ── Class declarations ────────────────────────────────────────────────

    /// Lower a `class Foo { ... }` declaration.
    ///
    /// Emits:
    ///   - `__class_Foo_constructor(p0: i64, …) -> i64`
    ///   - `__class_Foo_<method>(this: i64, p0: i64, …) -> i64`  for every method
    pub(super) fn lower_class_declaration(&mut self, class: &Class<'_>) -> Result<()> {
        let Some(id) = &class.id else { return Ok(()) };
        let class_name = id.name.to_string();

        self.lower_class_constructor(&class_name, class)?;

        for elem in &class.body.body {
            if let ClassElement::MethodDefinition(method) = elem {
                if method.kind != MethodDefinitionKind::Constructor {
                    self.lower_class_method(&class_name, method)?;
                }
            }
        }
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

        let i64_type  = self.i64_type();
        let func_name = format!("__class_{}_constructor", class_name);

        let n_params = constructor.map_or(0, |c| c.value.params.items.len());
        let param_specs: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            (0..n_params).map(|_| (i64_type, self.loc)).collect();
        let func_type = FunctionType::new(self.ctx, &vec![i64_type; n_params], &[i64_type]);

        let region = Region::new();
        let entry  = region.append_block(Block::new(&param_specs));

        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();

        // Allocate the new object and expose it as `this`.
        let this_val: Value<'_, '_> = entry
            .append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_obj_new"),
                &[],
                &[i64_type],
                self.loc,
            ))
            .result(0)?
            .into();
        scope.insert("this".to_string(), this_val);

        // Bind constructor parameters.
        if let Some(ctor) = constructor {
            for (i, param) in ctor.value.params.items.iter().enumerate() {
                if let BindingPattern::BindingIdentifier(id) = &param.pattern {
                    scope.insert(id.name.to_string(), entry.argument(i)?.into());
                }
            }
        }

        let mut current = entry;

        // Apply field initialisers (`x: number = 5`).
        for elem in &class.body.body {
            if let ClassElement::PropertyDefinition(prop) = elem {
                let Some(init_expr) = &prop.value else { continue };
                let key_str = match &prop.key {
                    oxc_ast::ast::PropertyKey::StaticIdentifier(id) => id.name.to_string(),
                    _ => continue,
                };
                let (val_opt, nb) = self.lower_expression(init_expr, current, &region, &mut scope)?;
                current = nb;
                if let Some(val) = val_opt {
                    let val_i64  = self.ensure_i64(val, current)?;
                    let key_ptr  = self.get_string_ptr(&key_str, current)?;
                    current.append_operation(func::call(
                        self.ctx,
                        FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
                        &[this_val, key_ptr, val_i64],
                        &[],
                        self.loc,
                    ));
                }
            }
        }

        // Lower constructor body (return type = i64 for class methods).
        self.fn_return_type = i64_type;
        if let Some(ctor) = constructor {
            if let Some(body) = &ctor.value.body {
                for stmt in &body.statements {
                    let (_, next) = self.lower_statement(stmt, current, &region, &mut scope, &[])?;
                    current = next;
                }
            }
        }
        self.fn_return_type = self.i32_type();

        if current.terminator().is_none() {
            current.append_operation(func::r#return(&[this_val], self.loc));
        }

        self.module.body().append_operation(func::func(
            self.ctx,
            StringAttribute::new(self.ctx, &func_name),
            TypeAttribute::new(func_type.into()),
            region,
            &[],
            self.loc,
        ));

        // Register so that `new Foo()` call sites can find the signature.
        self.funcs.insert(func_name, FuncSig {
            param_types: vec![i64_type; n_params],
            return_type: Some(i64_type),
        });
        Ok(())
    }

    // ── Non-constructor methods ────────────────────────────────────────────

    fn lower_class_method(&mut self, class_name: &str, method: &MethodDefinition<'_>) -> Result<()> {
        let Some(name) = method.key.static_name() else { return Ok(()) };
        let func_name = format!("__class_{}_{}", class_name, name);
        let i64_type  = self.i64_type();

        // `this` + explicit parameters, all i64 (TsVal).
        let n_params = method.value.params.items.len();
        let all_params = 1 + n_params;
        let param_specs: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            (0..all_params).map(|_| (i64_type, self.loc)).collect();
        let func_type = FunctionType::new(self.ctx, &vec![i64_type; all_params], &[i64_type]);

        let region = Region::new();
        let entry  = region.append_block(Block::new(&param_specs));

        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();
        scope.insert("this".to_string(), entry.argument(0)?.into());
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
                if let Some(v) = val {
                    result = v;
                }
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
}
