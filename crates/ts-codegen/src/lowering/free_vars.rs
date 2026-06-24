use super::*;

// ── Free-variable analysis ─────────────────────────────────────────────────────
// Walk arrow-function body to find outer-scope identifiers that must be captured.

pub(crate) type NameSet = std::collections::HashSet<String>;

pub(crate) fn collect_free_vars_stmts(
    stmts: &[oxc_ast::ast::Statement<'_>],
    params: &NameSet,
    outer_keys: &NameSet,
    out: &mut Vec<String>,
) {
    let mut local: NameSet = params.clone();
    for stmt in stmts { collect_locals_stmt(stmt, &mut local); }
    for stmt in stmts { collect_free_vars_stmt(stmt, params, &local, outer_keys, out); }
}

fn collect_locals_stmt(stmt: &oxc_ast::ast::Statement<'_>, locals: &mut NameSet) {
    use oxc_ast::ast::Statement;
    match stmt {
        Statement::VariableDeclaration(decl) => {
            for d in &decl.declarations {
                collect_locals_binding(&d.id, locals);
            }
        }
        Statement::FunctionDeclaration(f) => {
            if let Some(id) = &f.id { locals.insert(id.name.to_string()); }
        }
        _ => {}
    }
}

pub(crate) fn collect_locals_binding(pat: &oxc_ast::ast::BindingPattern<'_>, locals: &mut NameSet) {
    use oxc_ast::ast::BindingPattern;
    match pat {
        BindingPattern::BindingIdentifier(id) => { locals.insert(id.name.to_string()); }
        BindingPattern::ObjectPattern(op) => {
            for prop in &op.properties {
                collect_locals_binding(&prop.value, locals);
            }
        }
        BindingPattern::ArrayPattern(ap) => {
            for elem in ap.elements.iter().flatten() {
                collect_locals_binding(elem, locals);
            }
        }
        _ => {}
    }
}

/// Pre-insert a binding pattern's identifier names into scope with a placeholder value.
/// Used to make locally-declared variables visible to hoisted inner function declarations.
pub(crate) fn predeclare_binding<'c, 'b>(
    pat: &oxc_ast::ast::BindingPattern<'_>,
    placeholder: melior::ir::Value<'c, 'b>,
    scope: &mut HashMap<String, melior::ir::Value<'c, 'b>>,
) {
    use oxc_ast::ast::BindingPattern;
    match pat {
        BindingPattern::BindingIdentifier(id) => {
            scope.entry(id.name.to_string()).or_insert(placeholder);
        }
        BindingPattern::ObjectPattern(op) => {
            for prop in &op.properties {
                predeclare_binding(&prop.value, placeholder, scope);
            }
        }
        BindingPattern::ArrayPattern(ap) => {
            for elem in ap.elements.iter().flatten() {
                predeclare_binding(elem, placeholder, scope);
            }
        }
        _ => {}
    }
}

fn collect_free_vars_stmt(
    stmt: &oxc_ast::ast::Statement<'_>,
    params: &NameSet,
    locals: &NameSet,
    outer_keys: &NameSet,
    out: &mut Vec<String>,
) {
    use oxc_ast::ast::Statement;
    match stmt {
        Statement::ExpressionStatement(es) => collect_fv_expr(&es.expression, params, locals, outer_keys, out),
        Statement::ReturnStatement(rs) => {
            if let Some(arg) = &rs.argument {
                collect_fv_expr(arg, params, locals, outer_keys, out);
            }
        }
        Statement::VariableDeclaration(decl) => {
            for d in &decl.declarations {
                if let Some(init) = &d.init {
                    collect_fv_expr(init, params, locals, outer_keys, out);
                }
            }
        }
        Statement::IfStatement(if_stmt) => {
            collect_fv_expr(&if_stmt.test, params, locals, outer_keys, out);
            collect_free_vars_stmt(&if_stmt.consequent, params, locals, outer_keys, out);
            if let Some(alt) = &if_stmt.alternate {
                collect_free_vars_stmt(alt, params, locals, outer_keys, out);
            }
        }
        Statement::BlockStatement(block) => {
            for s in &block.body { collect_free_vars_stmt(s, params, locals, outer_keys, out); }
        }
        Statement::ForOfStatement(for_of) => {
            collect_fv_expr(&for_of.right, params, locals, outer_keys, out);
            collect_free_vars_stmt(&for_of.body, params, locals, outer_keys, out);
        }
        Statement::ForInStatement(for_in) => {
            collect_fv_expr(&for_in.right, params, locals, outer_keys, out);
            collect_free_vars_stmt(&for_in.body, params, locals, outer_keys, out);
        }
        Statement::ForStatement(for_stmt) => {
            if let Some(init) = &for_stmt.init {
                if let Some(expr) = init.as_expression() {
                    collect_fv_expr(expr, params, locals, outer_keys, out);
                } else if let oxc_ast::ast::ForStatementInit::VariableDeclaration(vd) = init {
                    for d in &vd.declarations {
                        if let Some(init_expr) = &d.init {
                            collect_fv_expr(init_expr, params, locals, outer_keys, out);
                        }
                    }
                }
            }
            if let Some(test) = &for_stmt.test {
                collect_fv_expr(test, params, locals, outer_keys, out);
            }
            if let Some(update) = &for_stmt.update {
                collect_fv_expr(update, params, locals, outer_keys, out);
            }
            collect_free_vars_stmt(&for_stmt.body, params, locals, outer_keys, out);
        }
        Statement::WhileStatement(w) => {
            collect_fv_expr(&w.test, params, locals, outer_keys, out);
            collect_free_vars_stmt(&w.body, params, locals, outer_keys, out);
        }
        Statement::TryStatement(try_stmt) => {
            for s in &try_stmt.block.body {
                collect_free_vars_stmt(s, params, locals, outer_keys, out);
            }
            if let Some(handler) = &try_stmt.handler {
                for s in &handler.body.body {
                    collect_free_vars_stmt(s, params, locals, outer_keys, out);
                }
            }
            if let Some(fin) = &try_stmt.finalizer {
                for s in &fin.body {
                    collect_free_vars_stmt(s, params, locals, outer_keys, out);
                }
            }
        }
        Statement::FunctionDeclaration(f) => {
            // Inner function declarations transitively capture outer vars.
            // Scan the inner body with the inner function's params + locals excluded.
            if let Some(body) = &f.body {
                let mut inner_locals = locals.clone();
                for param in &f.params.items {
                    if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &param.pattern {
                        inner_locals.insert(id.name.to_string());
                    }
                }
                if let Some(id) = &f.id { inner_locals.insert(id.name.to_string()); }
                for stmt in &body.statements { collect_locals_stmt(stmt, &mut inner_locals); }
                for stmt in &body.statements {
                    collect_free_vars_stmt(stmt, params, &inner_locals, outer_keys, out);
                }
            }
        }
        _ => {}
    }
}

fn collect_fv_expr(
    expr: &oxc_ast::ast::Expression<'_>,
    params: &NameSet,
    locals: &NameSet,
    outer_keys: &NameSet,
    out: &mut Vec<String>,
) {
    use oxc_ast::ast::Expression;
    match expr {
        Expression::Identifier(id) => {
            let name = id.name.as_str();
            if !params.contains(name) && !locals.contains(name) && outer_keys.contains(name) {
                if !out.contains(&name.to_string()) {
                    out.push(name.to_string());
                }
            }
        }
        Expression::ParenthesizedExpression(pe) => {
            collect_fv_expr(&pe.expression, params, locals, outer_keys, out);
        }
        Expression::ThisExpression(_) => {
            // Arrow functions capture `this` from the enclosing lexical scope.
            if outer_keys.contains("this") && !out.contains(&"this".to_string()) {
                out.push("this".to_string());
            }
        }
        Expression::BinaryExpression(bin) => {
            collect_fv_expr(&bin.left, params, locals, outer_keys, out);
            collect_fv_expr(&bin.right, params, locals, outer_keys, out);
        }
        Expression::LogicalExpression(log) => {
            collect_fv_expr(&log.left, params, locals, outer_keys, out);
            collect_fv_expr(&log.right, params, locals, outer_keys, out);
        }
        Expression::UnaryExpression(un) => {
            collect_fv_expr(&un.argument, params, locals, outer_keys, out);
        }
        Expression::AssignmentExpression(assign) => {
            collect_fv_expr(&assign.right, params, locals, outer_keys, out);
            // Also scan the LHS target for `this` (e.g. `this.#field ??= rhs`).
            collect_fv_assignment_target(&assign.left, params, locals, outer_keys, out);
        }
        Expression::CallExpression(call) => {
            collect_fv_expr(&call.callee, params, locals, outer_keys, out);
            for arg in &call.arguments {
                match arg {
                    oxc_ast::ast::Argument::SpreadElement(spread) => {
                        collect_fv_expr(&spread.argument, params, locals, outer_keys, out);
                    }
                    _ => {
                        if let Some(e) = arg.as_expression() {
                            collect_fv_expr(e, params, locals, outer_keys, out);
                        }
                    }
                }
            }
        }
        Expression::StaticMemberExpression(m) => {
            collect_fv_expr(&m.object, params, locals, outer_keys, out);
        }
        Expression::ComputedMemberExpression(m) => {
            collect_fv_expr(&m.object, params, locals, outer_keys, out);
            collect_fv_expr(&m.expression, params, locals, outer_keys, out);
        }
        Expression::ConditionalExpression(cond) => {
            collect_fv_expr(&cond.test, params, locals, outer_keys, out);
            collect_fv_expr(&cond.consequent, params, locals, outer_keys, out);
            collect_fv_expr(&cond.alternate, params, locals, outer_keys, out);
        }
        Expression::TemplateLiteral(tmpl) => {
            for e in &tmpl.expressions { collect_fv_expr(e, params, locals, outer_keys, out); }
        }
        Expression::TaggedTemplateExpression(tagged) => {
            collect_fv_expr(&tagged.tag, params, locals, outer_keys, out);
            for e in &tagged.quasi.expressions { collect_fv_expr(e, params, locals, outer_keys, out); }
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                match elem {
                    oxc_ast::ast::ArrayExpressionElement::SpreadElement(spread) => {
                        collect_fv_expr(&spread.argument, params, locals, outer_keys, out);
                    }
                    _ => {
                        if let Some(e) = elem.as_expression() {
                            collect_fv_expr(e, params, locals, outer_keys, out);
                        }
                    }
                }
            }
        }
        Expression::ObjectExpression(obj) => {
            use oxc_ast::ast::ObjectPropertyKind;
            for prop in &obj.properties {
                if let ObjectPropertyKind::ObjectProperty(p) = prop {
                    collect_fv_expr(&p.value, params, locals, outer_keys, out);
                }
            }
        }
        // Nested arrow functions: scan their bodies transitively.
        // Variables only used inside the inner arrow still need to be captured by the outer.
        Expression::ArrowFunctionExpression(arrow) => {
            let mut inner_locals = locals.clone();
            for p in &arrow.params.items {
                if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &p.pattern {
                    inner_locals.insert(id.name.to_string());
                }
            }
            if let Some(rest) = &arrow.params.rest {
                if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &rest.rest.argument {
                    inner_locals.insert(id.name.to_string());
                }
            }
            for stmt in &arrow.body.statements {
                collect_locals_stmt(stmt, &mut inner_locals);
            }
            for stmt in &arrow.body.statements {
                collect_free_vars_stmt(stmt, params, &inner_locals, outer_keys, out);
            }
        }
        Expression::FunctionExpression(func_expr) => {
            let mut inner_locals = locals.clone();
            if let Some(id) = &func_expr.id {
                inner_locals.insert(id.name.to_string());
            }
            for p in &func_expr.params.items {
                if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &p.pattern {
                    inner_locals.insert(id.name.to_string());
                }
            }
            if let Some(rest) = &func_expr.params.rest {
                if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &rest.rest.argument {
                    inner_locals.insert(id.name.to_string());
                }
            }
            if let Some(body) = func_expr.body.as_deref() {
                for stmt in &body.statements {
                    collect_locals_stmt(stmt, &mut inner_locals);
                }
                for stmt in &body.statements {
                    collect_free_vars_stmt(stmt, params, &inner_locals, outer_keys, out);
                }
            }
        }
        // TypeScript wrappers: look through to the inner expression
        Expression::TSAsExpression(ts_as) => {
            collect_fv_expr(&ts_as.expression, params, locals, outer_keys, out);
        }
        Expression::TSSatisfiesExpression(ts_sat) => {
            collect_fv_expr(&ts_sat.expression, params, locals, outer_keys, out);
        }
        Expression::TSTypeAssertion(ts_assert) => {
            collect_fv_expr(&ts_assert.expression, params, locals, outer_keys, out);
        }
        Expression::TSNonNullExpression(ts_nn) => {
            collect_fv_expr(&ts_nn.expression, params, locals, outer_keys, out);
        }
        Expression::AwaitExpression(aw) => {
            collect_fv_expr(&aw.argument, params, locals, outer_keys, out);
        }
        Expression::SequenceExpression(seq) => {
            for e in &seq.expressions { collect_fv_expr(e, params, locals, outer_keys, out); }
        }
        Expression::NewExpression(new_expr) => {
            collect_fv_expr(&new_expr.callee, params, locals, outer_keys, out);
            for arg in &new_expr.arguments {
                if let Some(e) = arg.as_expression() {
                    collect_fv_expr(e, params, locals, outer_keys, out);
                }
            }
        }
        Expression::PrivateFieldExpression(m) => {
            collect_fv_expr(&m.object, params, locals, outer_keys, out);
        }
        Expression::UpdateExpression(update) => {
            use oxc_ast::ast::SimpleAssignmentTarget;
            match &update.argument {
                SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                    let name = id.name.as_str();
                    if !params.contains(name) && !locals.contains(name) && outer_keys.contains(name) {
                        if !out.contains(&name.to_string()) { out.push(name.to_string()); }
                    }
                }
                SimpleAssignmentTarget::StaticMemberExpression(m) => {
                    collect_fv_expr(&m.object, params, locals, outer_keys, out);
                }
                SimpleAssignmentTarget::ComputedMemberExpression(m) => {
                    collect_fv_expr(&m.object, params, locals, outer_keys, out);
                    collect_fv_expr(&m.expression, params, locals, outer_keys, out);
                }
                SimpleAssignmentTarget::PrivateFieldExpression(m) => {
                    collect_fv_expr(&m.object, params, locals, outer_keys, out);
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn collect_fv_assignment_target(
    target: &oxc_ast::ast::AssignmentTarget<'_>,
    params: &NameSet,
    locals: &NameSet,
    outer_keys: &NameSet,
    out: &mut Vec<String>,
) {
    use oxc_ast::ast::AssignmentTarget;
    match target {
        AssignmentTarget::AssignmentTargetIdentifier(id) => {
            let name = id.name.as_str();
            if !params.contains(name) && !locals.contains(name) && outer_keys.contains(name) {
                if !out.contains(&name.to_string()) {
                    out.push(name.to_string());
                }
            }
        }
        AssignmentTarget::StaticMemberExpression(m) => {
            collect_fv_expr(&m.object, params, locals, outer_keys, out);
        }
        AssignmentTarget::ComputedMemberExpression(m) => {
            collect_fv_expr(&m.object, params, locals, outer_keys, out);
            collect_fv_expr(&m.expression, params, locals, outer_keys, out);
        }
        AssignmentTarget::PrivateFieldExpression(m) => {
            collect_fv_expr(&m.object, params, locals, outer_keys, out);
        }
        _ => {}
    }
}

// ── Mutable-capture cell analysis ────────────────────────────────────────────

/// Scan statements for assignments (=, +=, ||=, etc.) to variables in `target_vars`.
/// Does NOT recurse into nested closures (ArrowFunctionExpression / FunctionExpression).
fn scan_stmts_for_assignments(
    stmts: &[oxc_ast::ast::Statement<'_>],
    target_vars: &NameSet,
    result: &mut NameSet,
) {
    for stmt in stmts {
        scan_stmt_for_assignments(stmt, target_vars, result);
    }
}

fn scan_stmt_for_assignments(
    stmt: &oxc_ast::ast::Statement<'_>,
    target_vars: &NameSet,
    result: &mut NameSet,
) {
    use oxc_ast::ast::Statement;
    match stmt {
        Statement::ExpressionStatement(es) => scan_expr_for_assignments(&es.expression, target_vars, result),
        Statement::ReturnStatement(rs) => {
            if let Some(arg) = &rs.argument { scan_expr_for_assignments(arg, target_vars, result); }
        }
        Statement::VariableDeclaration(vd) => {
            for d in &vd.declarations {
                if let Some(init) = &d.init { scan_expr_for_assignments(init, target_vars, result); }
            }
        }
        Statement::IfStatement(if_stmt) => {
            scan_expr_for_assignments(&if_stmt.test, target_vars, result);
            scan_stmt_for_assignments(&if_stmt.consequent, target_vars, result);
            if let Some(alt) = &if_stmt.alternate { scan_stmt_for_assignments(alt, target_vars, result); }
        }
        Statement::BlockStatement(block) => {
            for s in &block.body { scan_stmt_for_assignments(s, target_vars, result); }
        }
        Statement::ForStatement(f) => {
            if let Some(test) = &f.test { scan_expr_for_assignments(test, target_vars, result); }
            if let Some(update) = &f.update { scan_expr_for_assignments(update, target_vars, result); }
            scan_stmt_for_assignments(&f.body, target_vars, result);
        }
        Statement::ForOfStatement(f) => { scan_stmt_for_assignments(&f.body, target_vars, result); }
        Statement::ForInStatement(f) => { scan_stmt_for_assignments(&f.body, target_vars, result); }
        Statement::WhileStatement(w) => {
            scan_expr_for_assignments(&w.test, target_vars, result);
            scan_stmt_for_assignments(&w.body, target_vars, result);
        }
        Statement::TryStatement(t) => {
            for s in &t.block.body { scan_stmt_for_assignments(s, target_vars, result); }
            if let Some(h) = &t.handler {
                for s in &h.body.body { scan_stmt_for_assignments(s, target_vars, result); }
            }
            if let Some(f) = &t.finalizer {
                for s in &f.body { scan_stmt_for_assignments(s, target_vars, result); }
            }
        }
        _ => {}
    }
}

fn scan_expr_for_assignments(
    expr: &oxc_ast::ast::Expression<'_>,
    target_vars: &NameSet,
    result: &mut NameSet,
) {
    use oxc_ast::ast::Expression;
    match expr {
        Expression::AssignmentExpression(ae) => {
            match &ae.left {
                oxc_ast::ast::AssignmentTarget::AssignmentTargetIdentifier(id) => {
                    if target_vars.contains(id.name.as_str()) { result.insert(id.name.to_string()); }
                }
                _ => {}
            }
            scan_expr_for_assignments(&ae.right, target_vars, result);
        }
        Expression::UpdateExpression(ue) => {
            if let oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = &ue.argument {
                if target_vars.contains(id.name.as_str()) { result.insert(id.name.to_string()); }
            }
        }
        Expression::CallExpression(call) => {
            scan_expr_for_assignments(&call.callee, target_vars, result);
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() { scan_expr_for_assignments(e, target_vars, result); }
            }
        }
        Expression::ConditionalExpression(c) => {
            scan_expr_for_assignments(&c.test, target_vars, result);
            scan_expr_for_assignments(&c.consequent, target_vars, result);
            scan_expr_for_assignments(&c.alternate, target_vars, result);
        }
        Expression::LogicalExpression(l) => {
            scan_expr_for_assignments(&l.left, target_vars, result);
            scan_expr_for_assignments(&l.right, target_vars, result);
        }
        Expression::BinaryExpression(b) => {
            scan_expr_for_assignments(&b.left, target_vars, result);
            scan_expr_for_assignments(&b.right, target_vars, result);
        }
        Expression::ParenthesizedExpression(pe) => {
            scan_expr_for_assignments(&pe.expression, target_vars, result);
        }
        Expression::UnaryExpression(ue) => {
            scan_expr_for_assignments(&ue.argument, target_vars, result);
        }
        Expression::SequenceExpression(s) => {
            for e in &s.expressions { scan_expr_for_assignments(e, target_vars, result); }
        }
        Expression::ObjectExpression(obj) => {
            use oxc_ast::ast::ObjectPropertyKind;
            for prop in &obj.properties {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => scan_expr_for_assignments(&p.value, target_vars, result),
                    ObjectPropertyKind::SpreadProperty(s) => scan_expr_for_assignments(&s.argument, target_vars, result),
                }
            }
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                if let Some(e) = elem.as_expression() { scan_expr_for_assignments(e, target_vars, result); }
            }
        }
        // DO NOT recurse into closures — they have their own scope
        Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_) => {}
        _ => {}
    }
}

/// Walk statements/expressions looking for directly-nested closures.
/// For each closure found, scan its body for assignments to `outer_vars`.
/// Returns the set of outer_vars that are assigned in any directly-nested closure.
fn find_vars_mutated_in_closures(
    stmts: &[oxc_ast::ast::Statement<'_>],
    outer_vars: &NameSet,
    result: &mut NameSet,
) {
    for stmt in stmts {
        fvmic_stmt(stmt, outer_vars, result);
    }
}

fn fvmic_stmt(stmt: &oxc_ast::ast::Statement<'_>, outer_vars: &NameSet, result: &mut NameSet) {
    use oxc_ast::ast::Statement;
    match stmt {
        Statement::ExpressionStatement(es) => fvmic_expr(&es.expression, outer_vars, result),
        Statement::ReturnStatement(rs) => {
            if let Some(arg) = &rs.argument { fvmic_expr(arg, outer_vars, result); }
        }
        Statement::VariableDeclaration(vd) => {
            for d in &vd.declarations {
                if let Some(init) = &d.init { fvmic_expr(init, outer_vars, result); }
            }
        }
        Statement::IfStatement(if_stmt) => {
            fvmic_expr(&if_stmt.test, outer_vars, result);
            fvmic_stmt(&if_stmt.consequent, outer_vars, result);
            if let Some(alt) = &if_stmt.alternate { fvmic_stmt(alt, outer_vars, result); }
        }
        Statement::BlockStatement(block) => {
            for s in &block.body { fvmic_stmt(s, outer_vars, result); }
        }
        Statement::ForStatement(f) => {
            if let Some(test) = &f.test { fvmic_expr(test, outer_vars, result); }
            if let Some(update) = &f.update { fvmic_expr(update, outer_vars, result); }
            fvmic_stmt(&f.body, outer_vars, result);
        }
        Statement::ForOfStatement(f) => {
            fvmic_expr(&f.right, outer_vars, result);
            fvmic_stmt(&f.body, outer_vars, result);
        }
        Statement::ForInStatement(f) => { fvmic_stmt(&f.body, outer_vars, result); }
        Statement::WhileStatement(w) => {
            fvmic_expr(&w.test, outer_vars, result);
            fvmic_stmt(&w.body, outer_vars, result);
        }
        Statement::TryStatement(t) => {
            for s in &t.block.body { fvmic_stmt(s, outer_vars, result); }
            if let Some(h) = &t.handler {
                for s in &h.body.body { fvmic_stmt(s, outer_vars, result); }
            }
            if let Some(f) = &t.finalizer {
                for s in &f.body { fvmic_stmt(s, outer_vars, result); }
            }
        }
        _ => {}
    }
}

fn fvmic_expr(expr: &oxc_ast::ast::Expression<'_>, outer_vars: &NameSet, result: &mut NameSet) {
    use oxc_ast::ast::Expression;
    match expr {
        Expression::ArrowFunctionExpression(arrow) => {
            // Found a closure: scan its body for assignments to outer_vars
            let mut closure_locals: NameSet = NameSet::new();
            for p in &arrow.params.items {
                if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &p.pattern {
                    closure_locals.insert(id.name.to_string());
                }
            }
            for stmt in &arrow.body.statements { collect_locals_stmt(stmt, &mut closure_locals); }
            let effective: NameSet = outer_vars.iter()
                .filter(|v| !closure_locals.contains(*v))
                .cloned().collect();
            scan_stmts_for_assignments(&arrow.body.statements, &effective, result);
        }
        Expression::FunctionExpression(f) => {
            if let Some(body) = &f.body {
                let mut closure_locals: NameSet = NameSet::new();
                for p in &f.params.items {
                    if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &p.pattern {
                        closure_locals.insert(id.name.to_string());
                    }
                }
                for stmt in &body.statements { collect_locals_stmt(stmt, &mut closure_locals); }
                let effective: NameSet = outer_vars.iter()
                    .filter(|v| !closure_locals.contains(*v))
                    .cloned().collect();
                scan_stmts_for_assignments(&body.statements, &effective, result);
            }
        }
        // Recurse into non-closure expressions to find nested closures
        Expression::CallExpression(call) => {
            fvmic_expr(&call.callee, outer_vars, result);
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() { fvmic_expr(e, outer_vars, result); }
            }
        }
        Expression::AssignmentExpression(ae) => { fvmic_expr(&ae.right, outer_vars, result); }
        Expression::BinaryExpression(b) => {
            fvmic_expr(&b.left, outer_vars, result);
            fvmic_expr(&b.right, outer_vars, result);
        }
        Expression::LogicalExpression(l) => {
            fvmic_expr(&l.left, outer_vars, result);
            fvmic_expr(&l.right, outer_vars, result);
        }
        Expression::ConditionalExpression(c) => {
            fvmic_expr(&c.test, outer_vars, result);
            fvmic_expr(&c.consequent, outer_vars, result);
            fvmic_expr(&c.alternate, outer_vars, result);
        }
        Expression::ArrayExpression(arr) => {
            for elem in &arr.elements {
                if let Some(e) = elem.as_expression() { fvmic_expr(e, outer_vars, result); }
            }
        }
        Expression::ObjectExpression(obj) => {
            use oxc_ast::ast::ObjectPropertyKind;
            for prop in &obj.properties {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => fvmic_expr(&p.value, outer_vars, result),
                    ObjectPropertyKind::SpreadProperty(s) => fvmic_expr(&s.argument, outer_vars, result),
                }
            }
        }
        Expression::ParenthesizedExpression(pe) => { fvmic_expr(&pe.expression, outer_vars, result); }
        Expression::StaticMemberExpression(m) => { fvmic_expr(&m.object, outer_vars, result); }
        Expression::ComputedMemberExpression(m) => {
            fvmic_expr(&m.object, outer_vars, result);
            fvmic_expr(&m.expression, outer_vars, result);
        }
        Expression::SequenceExpression(s) => {
            for e in &s.expressions { fvmic_expr(e, outer_vars, result); }
        }
        Expression::TaggedTemplateExpression(tagged) => {
            fvmic_expr(&tagged.tag, outer_vars, result);
            for e in &tagged.quasi.expressions { fvmic_expr(e, outer_vars, result); }
        }
        _ => {}
    }
}

/// Compute the set of local variables in `stmts` that are mutated in directly-nested closures.
/// These variables need to be "cell-ified" (wrapped in a single-element TsArray) so that
/// mutations inside closures are visible to the outer scope after the closure runs.
pub(crate) fn compute_cell_vars_for_body(stmts: &[oxc_ast::ast::Statement<'_>]) -> NameSet {
    let mut local_vars: NameSet = NameSet::new();
    for stmt in stmts { collect_locals_stmt(stmt, &mut local_vars); }
    let mut cell_vars: NameSet = NameSet::new();
    find_vars_mutated_in_closures(stmts, &local_vars, &mut cell_vars);
    cell_vars
}

/// Returns `true` if the statement list contains a direct reference to `arguments`
/// (not inside a nested function declaration or arrow function, which have their own scope).
pub(crate) fn body_uses_arguments(stmts: &[oxc_ast::ast::Statement<'_>]) -> bool {
    use oxc_ast::ast::{Statement, Expression, BindingPattern};
    for stmt in stmts {
        if stmt_uses_arguments(stmt) { return true; }
    }
    false
}

fn stmt_uses_arguments(stmt: &oxc_ast::ast::Statement<'_>) -> bool {
    use oxc_ast::ast::Statement;
    match stmt {
        Statement::ExpressionStatement(e) => expr_uses_arguments(&e.expression),
        Statement::ReturnStatement(r) => r.argument.as_ref().map_or(false, |e| expr_uses_arguments(e)),
        Statement::IfStatement(i) => {
            expr_uses_arguments(&i.test)
                || stmt_uses_arguments(&i.consequent)
                || i.alternate.as_ref().map_or(false, |s| stmt_uses_arguments(s))
        }
        Statement::BlockStatement(b) => b.body.iter().any(stmt_uses_arguments),
        Statement::VariableDeclaration(v) => v.declarations.iter().any(|d| {
            d.init.as_ref().map_or(false, |e| expr_uses_arguments(e))
        }),
        Statement::ForStatement(f) => {
            f.test.as_ref().map_or(false, |e| expr_uses_arguments(e))
                || f.update.as_ref().map_or(false, |e| expr_uses_arguments(e))
                || stmt_uses_arguments(&f.body)
        }
        Statement::WhileStatement(w) => expr_uses_arguments(&w.test) || stmt_uses_arguments(&w.body),
        Statement::ThrowStatement(t) => expr_uses_arguments(&t.argument),
        Statement::TryStatement(t) => {
            t.block.body.iter().any(stmt_uses_arguments)
                || t.handler.as_ref().map_or(false, |h| h.body.body.iter().any(stmt_uses_arguments))
                || t.finalizer.as_ref().map_or(false, |f| f.body.iter().any(stmt_uses_arguments))
        }
        _ => false,
    }
}

fn expr_uses_arguments(expr: &oxc_ast::ast::Expression<'_>) -> bool {
    use oxc_ast::ast::Expression;
    match expr {
        Expression::Identifier(id) => id.name == "arguments",
        Expression::CallExpression(c) => {
            expr_uses_arguments(&c.callee)
                || c.arguments.iter().any(|a| a.as_expression().map_or(false, expr_uses_arguments))
        }
        Expression::BinaryExpression(b) => expr_uses_arguments(&b.left) || expr_uses_arguments(&b.right),
        Expression::LogicalExpression(l) => expr_uses_arguments(&l.left) || expr_uses_arguments(&l.right),
        Expression::StaticMemberExpression(s) => expr_uses_arguments(&s.object),
        Expression::ComputedMemberExpression(c) => {
            expr_uses_arguments(&c.object) || expr_uses_arguments(&c.expression)
        }
        Expression::AssignmentExpression(a) => expr_uses_arguments(&a.right),
        Expression::ConditionalExpression(c) => {
            expr_uses_arguments(&c.test) || expr_uses_arguments(&c.consequent) || expr_uses_arguments(&c.alternate)
        }
        // Arrow functions and function expressions do NOT share `arguments` — stop here.
        Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_) => false,
        Expression::ArrayExpression(a) => a.elements.iter().any(|e| {
            use oxc_ast::ast::ArrayExpressionElement;
            match e {
                ArrayExpressionElement::SpreadElement(s) => expr_uses_arguments(&s.argument),
                _ => e.as_expression().map_or(false, expr_uses_arguments),
            }
        }),
        Expression::ObjectExpression(o) => o.properties.iter().any(|p| {
            use oxc_ast::ast::ObjectPropertyKind;
            match p {
                ObjectPropertyKind::ObjectProperty(op) => expr_uses_arguments(&op.value),
                _ => false,
            }
        }),
        Expression::UnaryExpression(u) => expr_uses_arguments(&u.argument),
        Expression::UpdateExpression(_) => false,
        Expression::TemplateLiteral(t) => t.expressions.iter().any(expr_uses_arguments),
        Expression::TaggedTemplateExpression(t) => {
            expr_uses_arguments(&t.tag) || t.quasi.expressions.iter().any(expr_uses_arguments)
        }
        _ => false,
    }
}

