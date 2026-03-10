use super::*;
use oxc_ast::ast::{TSEnumDeclaration, TSEnumMemberName};

impl<'c, 'm> Lowerer<'c, 'm> {
    /// Collect all enum declarations in the program into `self.enums`.
    pub(super) fn collect_enum_definitions(&mut self, program: &Program<'_>) {
        for stmt in &program.body {
            self.collect_enum_from_stmt(stmt);
        }
    }

    fn collect_enum_from_stmt(&mut self, stmt: &Statement<'_>) {
        match stmt {
            Statement::TSEnumDeclaration(e) => self.register_enum(e),
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(Declaration::TSEnumDeclaration(e)) = &exp.declaration {
                    self.register_enum(e);
                }
            }
            _ => {}
        }
    }

    fn register_enum(&mut self, enum_decl: &TSEnumDeclaration<'_>) {
        let name = enum_decl.id.name.to_string();
        let mut members = HashMap::new();
        let mut next_val: i64 = 0;
        for member in &enum_decl.body.members {
            let key = match &member.id {
                TSEnumMemberName::Identifier(id) => id.name.to_string(),
                TSEnumMemberName::String(s) => s.value.to_string(),
                _ => continue,
            };
            let val = if let Some(init) = &member.initializer {
                if let Expression::NumericLiteral(n) = init {
                    n.value as i64
                } else {
                    next_val
                }
            } else {
                next_val
            };
            members.insert(key, val);
            next_val = val + 1;
        }
        self.enums.insert(name, members);
    }

    /// Lower an enum declaration as a runtime TsObject bound to the enum name.
    pub(super) fn lower_enum_declaration<'b>(
        &mut self,
        enum_decl: &TSEnumDeclaration<'_>,
        mut block: BlockRef<'c, 'b>,
        _region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let name = enum_decl.id.name.to_string();
        let i64_type = self.i64_type();

        // %obj = ts_obj_new()
        let obj_val: Value<'c, 'b> = block
            .append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_obj_new"),
                &[],
                &[i64_type],
                self.loc,
            ))
            .result(0)?
            .into();

        let mut next_val: i64 = 0;
        for member in &enum_decl.body.members {
            let key_str = match &member.id {
                TSEnumMemberName::Identifier(id) => id.name.to_string(),
                TSEnumMemberName::String(s) => s.value.to_string(),
                _ => continue,
            };
            let val = if let Some(init) = &member.initializer {
                if let Expression::NumericLiteral(n) = init {
                    n.value as i64
                } else {
                    next_val
                }
            } else {
                next_val
            };
            next_val = val + 1;

            let num_val = self.lower_numeric_literal(val, block)?;
            let num_i64 = self.ensure_i64(num_val, block)?;
            let key_ptr = self.get_string_ptr(&key_str, block)?;

            block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
                &[obj_val, key_ptr, num_i64],
                &[],
                self.loc,
            ));

            // Also set reverse mapping: number → name string
            let name_val = self.lower_string_literal(&key_str, block)?;
            let num_key_str = val.to_string();
            let num_key_ptr = self.get_string_ptr(&num_key_str, block)?;
            block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_obj_set"),
                &[obj_val, num_key_ptr, name_val],
                &[],
                self.loc,
            ));
        }

        scope.insert(name, obj_val);
        Ok((None, block))
    }
}
