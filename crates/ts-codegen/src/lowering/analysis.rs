use super::*;

// ── Scalar variable pre-pass ─────────────────────────────────────────────────

/// Compute the set of local variables in `stmts` that are provably always scalar
/// (TAG_INT, TAG_BOOL, TAG_NULL, TAG_UNDEFINED — never a heap pointer) throughout the
/// function body.  Both `const` and `let`/`var` declarations are considered; a variable
/// is disqualified if any assignment to it may produce a heap value.
///
/// Cell variables MUST be excluded by the caller (they are TsArrays = heap objects).
pub(crate) fn compute_scalar_vars_for_body(stmts: &[oxc_ast::ast::Statement<'_>]) -> NameSet {
    let mut candidates: NameSet = NameSet::new();
    let mut disqualified: NameSet = NameSet::new();
    for stmt in stmts {
        sv_collect_stmt(stmt, &mut candidates, &mut disqualified);
    }
    // Fixpoint: re-run disqualification using first-pass survivors as the "known scalar" context.
    // `known_scalars` = variables that survived pass 1 without disqualification.
    // This handles `sum += i` where `i` is a surviving scalar loop counter —
    // `i` is in known_scalars so `sum += i` does NOT disqualify `sum`.
    // Using surviving (not raw) candidates avoids the false-positive where
    // `x = y` and `y` was disqualified in pass 1 but still appeared in raw candidates.
    let known_scalars: NameSet = candidates.iter()
        .filter(|v| !disqualified.contains(*v))
        .cloned()
        .collect();
    let mut disqualified2: NameSet = NameSet::new();
    for stmt in stmts {
        sv_collect_stmt_with_context(stmt, &candidates, &known_scalars, &mut disqualified2);
    }
    candidates.retain(|v| !disqualified2.contains(v));
    candidates
}

/// Walk a statement, collecting scalar candidates (variable declarations with scalar inits)
/// and disqualifying any variable whose name gets a non-scalar assignment.
/// Does NOT recurse into nested function declarations or function/arrow expressions.
fn sv_collect_stmt(
    stmt: &oxc_ast::ast::Statement<'_>,
    cands: &mut NameSet,
    dis: &mut NameSet,
) {
    use oxc_ast::ast::{Statement, ForStatementInit, ForStatementLeft};
    match stmt {
        Statement::VariableDeclaration(vd) => {
            for decl in &vd.declarations {
                if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &decl.id {
                    let name = id.name.to_string();
                    let is_scalar = decl.init.as_ref()
                        .map(|e| crate::lowering::expr_is_definitely_scalar(e))
                        .unwrap_or(false); // no init → unknown value, don't assume scalar
                    if is_scalar { cands.insert(name); } else { dis.insert(name); }
                }
                // Destructuring patterns: too complex, skip
            }
        }
        Statement::ExpressionStatement(es) => sv_scan_expr(&es.expression, cands, dis),
        Statement::ReturnStatement(r) => {
            if let Some(e) = &r.argument { sv_scan_expr(e, cands, dis); }
        }
        Statement::IfStatement(s) => {
            sv_scan_expr(&s.test, cands, dis);
            sv_collect_stmt(&s.consequent, cands, dis);
            if let Some(alt) = &s.alternate { sv_collect_stmt(alt, cands, dis); }
        }
        Statement::BlockStatement(b) => {
            for s in &b.body { sv_collect_stmt(s, cands, dis); }
        }
        Statement::WhileStatement(s) => {
            sv_scan_expr(&s.test, cands, dis);
            sv_collect_stmt(&s.body, cands, dis);
        }
        Statement::DoWhileStatement(s) => {
            sv_scan_expr(&s.test, cands, dis);
            sv_collect_stmt(&s.body, cands, dis);
        }
        Statement::ForStatement(s) => {
            if let Some(init) = &s.init {
                match init {
                    ForStatementInit::VariableDeclaration(vd) => {
                        for decl in &vd.declarations {
                            if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &decl.id {
                                let name = id.name.to_string();
                                let is_scalar = decl.init.as_ref()
                                    .map(|e| crate::lowering::expr_is_definitely_scalar(e))
                                    .unwrap_or(false); // no init → unknown value, don't assume scalar
                                if is_scalar { cands.insert(name); } else { dis.insert(name); }
                            }
                        }
                    }
                    other => {
                        if let Some(e) = other.as_expression() { sv_scan_expr(e, cands, dis); }
                    }
                }
            }
            if let Some(test) = &s.test { sv_scan_expr(test, cands, dis); }
            if let Some(upd) = &s.update { sv_scan_expr(upd, cands, dis); }
            sv_collect_stmt(&s.body, cands, dis);
        }
        Statement::ForOfStatement(s) => {
            // for-of iterator values could be anything — disqualify the loop variable
            if let ForStatementLeft::VariableDeclaration(vd) = &s.left {
                for decl in &vd.declarations {
                    if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &decl.id {
                        dis.insert(id.name.to_string());
                    }
                }
            }
            sv_scan_expr(&s.right, cands, dis);
            sv_collect_stmt(&s.body, cands, dis);
        }
        Statement::ForInStatement(s) => {
            // for-in key is a string (heap) — disqualify
            if let ForStatementLeft::VariableDeclaration(vd) = &s.left {
                for decl in &vd.declarations {
                    if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &decl.id {
                        dis.insert(id.name.to_string());
                    }
                }
            }
            sv_scan_expr(&s.right, cands, dis);
            sv_collect_stmt(&s.body, cands, dis);
        }
        Statement::SwitchStatement(s) => {
            sv_scan_expr(&s.discriminant, cands, dis);
            for case in &s.cases {
                if let Some(test) = &case.test { sv_scan_expr(test, cands, dis); }
                for cs in &case.consequent { sv_collect_stmt(cs, cands, dis); }
            }
        }
        Statement::TryStatement(s) => {
            for cs in &s.block.body { sv_collect_stmt(cs, cands, dis); }
            if let Some(handler) = &s.handler {
                for cs in &handler.body.body { sv_collect_stmt(cs, cands, dis); }
            }
            if let Some(fin) = &s.finalizer {
                for cs in &fin.body { sv_collect_stmt(cs, cands, dis); }
            }
        }
        Statement::ThrowStatement(s) => sv_scan_expr(&s.argument, cands, dis),
        Statement::LabeledStatement(s) => sv_collect_stmt(&s.body, cands, dis),
        // Function declarations: separate scope, do NOT recurse
        Statement::FunctionDeclaration(_) => {}
        _ => {}
    }
}

/// Scan an expression for assignments that could disqualify scalar candidates.
/// Does NOT recurse into arrow functions or function expressions (separate scopes).
fn sv_scan_expr(
    expr: &oxc_ast::ast::Expression<'_>,
    cands: &mut NameSet,
    dis: &mut NameSet,
) {
    use oxc_ast::ast::{Expression, AssignmentOperator};
    match expr {
        Expression::AssignmentExpression(a) => {
            sv_scan_expr(&a.right, cands, dis);
            if let oxc_ast::ast::AssignmentTarget::AssignmentTargetIdentifier(id) = &a.left {
                let name = id.name.to_string();
                if cands.contains(&name) {
                    let rhs_scalar = crate::lowering::expr_is_definitely_scalar(&a.right);
                    let disqualify = match a.operator {
                        AssignmentOperator::Assign   => !rhs_scalar,
                        // += may produce a string if RHS is non-scalar
                        AssignmentOperator::Addition => !rhs_scalar,
                        // All other compound ops coerce to numeric — always scalar result
                        _ => false,
                    };
                    if disqualify { dis.insert(name); }
                }
            }
        }
        Expression::BinaryExpression(b) => { sv_scan_expr(&b.left, cands, dis); sv_scan_expr(&b.right, cands, dis); }
        Expression::LogicalExpression(l) => { sv_scan_expr(&l.left, cands, dis); sv_scan_expr(&l.right, cands, dis); }
        Expression::UnaryExpression(u)   => sv_scan_expr(&u.argument, cands, dis),
        Expression::CallExpression(c) => {
            sv_scan_expr(&c.callee, cands, dis);
            for arg in &c.arguments {
                if let Some(e) = arg.as_expression() { sv_scan_expr(e, cands, dis); }
            }
        }
        Expression::StaticMemberExpression(m) => sv_scan_expr(&m.object, cands, dis),
        Expression::ComputedMemberExpression(m) => {
            sv_scan_expr(&m.object, cands, dis);
            sv_scan_expr(&m.expression, cands, dis);
        }
        Expression::ConditionalExpression(c) => {
            sv_scan_expr(&c.test, cands, dis);
            sv_scan_expr(&c.consequent, cands, dis);
            sv_scan_expr(&c.alternate, cands, dis);
        }
        Expression::SequenceExpression(s) => {
            for e in &s.expressions { sv_scan_expr(e, cands, dis); }
        }
        Expression::ArrayExpression(a) => {
            for el in &a.elements {
                if let Some(e) = el.as_expression() { sv_scan_expr(e, cands, dis); }
            }
        }
        Expression::ObjectExpression(o) => {
            use oxc_ast::ast::ObjectPropertyKind;
            for prop in &o.properties {
                if let ObjectPropertyKind::ObjectProperty(p) = prop { sv_scan_expr(&p.value, cands, dis); }
            }
        }
        Expression::TemplateLiteral(t) => {
            for e in &t.expressions { sv_scan_expr(e, cands, dis); }
        }
        Expression::NewExpression(n) => {
            for arg in &n.arguments {
                if let Some(e) = arg.as_expression() { sv_scan_expr(e, cands, dis); }
            }
        }
        // Arrow functions and function expressions are separate scopes — stop here
        Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_) => {}
        _ => {}
    }
}

/// Context-aware version of `sv_collect_stmt`.
/// `candidates` = all candidate target variables (used to decide whether to check an assignment).
/// `known_scalars` = first-pass survivors (variables confirmed scalar — used for RHS lookup).
/// This is the second pass of the fixpoint loop.
fn sv_collect_stmt_with_context(
    stmt: &oxc_ast::ast::Statement<'_>,
    candidates: &NameSet,
    known_scalars: &NameSet,
    dis: &mut NameSet,
) {
    use oxc_ast::ast::{Statement, ForStatementInit};
    match stmt {
        Statement::VariableDeclaration(_) => {}  // Already handled in first pass
        Statement::ExpressionStatement(es) => sv_scan_expr_with_context(&es.expression, candidates, known_scalars, dis),
        Statement::ReturnStatement(r) => {
            if let Some(e) = &r.argument { sv_scan_expr_with_context(e, candidates, known_scalars, dis); }
        }
        Statement::IfStatement(s) => {
            sv_scan_expr_with_context(&s.test, candidates, known_scalars, dis);
            sv_collect_stmt_with_context(&s.consequent, candidates, known_scalars, dis);
            if let Some(alt) = &s.alternate { sv_collect_stmt_with_context(alt, candidates, known_scalars, dis); }
        }
        Statement::BlockStatement(b) => {
            for s in &b.body { sv_collect_stmt_with_context(s, candidates, known_scalars, dis); }
        }
        Statement::WhileStatement(s) => {
            sv_scan_expr_with_context(&s.test, candidates, known_scalars, dis);
            sv_collect_stmt_with_context(&s.body, candidates, known_scalars, dis);
        }
        Statement::DoWhileStatement(s) => {
            sv_scan_expr_with_context(&s.test, candidates, known_scalars, dis);
            sv_collect_stmt_with_context(&s.body, candidates, known_scalars, dis);
        }
        Statement::ForStatement(s) => {
            if let Some(init) = &s.init {
                if let ForStatementInit::VariableDeclaration(_) = init {} // already handled
                else if let Some(e) = init.as_expression() { sv_scan_expr_with_context(e, candidates, known_scalars, dis); }
            }
            if let Some(test) = &s.test { sv_scan_expr_with_context(test, candidates, known_scalars, dis); }
            if let Some(upd) = &s.update { sv_scan_expr_with_context(upd, candidates, known_scalars, dis); }
            sv_collect_stmt_with_context(&s.body, candidates, known_scalars, dis);
        }
        Statement::ForOfStatement(s) => {
            sv_scan_expr_with_context(&s.right, candidates, known_scalars, dis);
            sv_collect_stmt_with_context(&s.body, candidates, known_scalars, dis);
        }
        Statement::ForInStatement(s) => {
            sv_scan_expr_with_context(&s.right, candidates, known_scalars, dis);
            sv_collect_stmt_with_context(&s.body, candidates, known_scalars, dis);
        }
        Statement::SwitchStatement(s) => {
            sv_scan_expr_with_context(&s.discriminant, candidates, known_scalars, dis);
            for case in &s.cases {
                if let Some(test) = &case.test { sv_scan_expr_with_context(test, candidates, known_scalars, dis); }
                for cs in &case.consequent { sv_collect_stmt_with_context(cs, candidates, known_scalars, dis); }
            }
        }
        Statement::TryStatement(s) => {
            for cs in &s.block.body { sv_collect_stmt_with_context(cs, candidates, known_scalars, dis); }
            if let Some(handler) = &s.handler {
                for cs in &handler.body.body { sv_collect_stmt_with_context(cs, candidates, known_scalars, dis); }
            }
            if let Some(fin) = &s.finalizer {
                for cs in &fin.body { sv_collect_stmt_with_context(cs, candidates, known_scalars, dis); }
            }
        }
        Statement::ThrowStatement(s) => sv_scan_expr_with_context(&s.argument, candidates, known_scalars, dis),
        Statement::LabeledStatement(s) => sv_collect_stmt_with_context(&s.body, candidates, known_scalars, dis),
        Statement::FunctionDeclaration(_) => {}
        _ => {}
    }
}

/// Context-aware expression scanner.
/// `candidates` = assignment targets to check (all first-pass candidates).
/// `known_scalars` = first-pass survivors used for RHS scalar lookup.
/// Prevents false disqualifications for patterns like `sum += i` where `i` is a known scalar.
fn sv_scan_expr_with_context(
    expr: &oxc_ast::ast::Expression<'_>,
    candidates: &NameSet,
    known_scalars: &NameSet,
    dis: &mut NameSet,
) {
    use oxc_ast::ast::{Expression, AssignmentOperator};
    match expr {
        Expression::AssignmentExpression(a) => {
            sv_scan_expr_with_context(&a.right, candidates, known_scalars, dis);
            if let oxc_ast::ast::AssignmentTarget::AssignmentTargetIdentifier(id) = &a.left {
                let name = id.name.to_string();
                if candidates.contains(&name) {
                    let rhs_scalar = expr_result_is_scalar_with_context(&a.right, known_scalars);
                    let disqualify = match a.operator {
                        AssignmentOperator::Assign   => !rhs_scalar,
                        AssignmentOperator::Addition => !rhs_scalar,
                        _ => false,
                    };
                    if disqualify { dis.insert(name); }
                }
            }
        }
        Expression::BinaryExpression(b) => {
            sv_scan_expr_with_context(&b.left, candidates, known_scalars, dis);
            sv_scan_expr_with_context(&b.right, candidates, known_scalars, dis);
        }
        Expression::LogicalExpression(l) => {
            sv_scan_expr_with_context(&l.left, candidates, known_scalars, dis);
            sv_scan_expr_with_context(&l.right, candidates, known_scalars, dis);
        }
        Expression::UnaryExpression(u) => sv_scan_expr_with_context(&u.argument, candidates, known_scalars, dis),
        Expression::CallExpression(c) => {
            sv_scan_expr_with_context(&c.callee, candidates, known_scalars, dis);
            for arg in &c.arguments {
                if let Some(e) = arg.as_expression() { sv_scan_expr_with_context(e, candidates, known_scalars, dis); }
            }
        }
        Expression::StaticMemberExpression(m) => sv_scan_expr_with_context(&m.object, candidates, known_scalars, dis),
        Expression::ComputedMemberExpression(m) => {
            sv_scan_expr_with_context(&m.object, candidates, known_scalars, dis);
            sv_scan_expr_with_context(&m.expression, candidates, known_scalars, dis);
        }
        Expression::ConditionalExpression(c) => {
            sv_scan_expr_with_context(&c.test, candidates, known_scalars, dis);
            sv_scan_expr_with_context(&c.consequent, candidates, known_scalars, dis);
            sv_scan_expr_with_context(&c.alternate, candidates, known_scalars, dis);
        }
        Expression::SequenceExpression(s) => {
            for e in &s.expressions { sv_scan_expr_with_context(e, candidates, known_scalars, dis); }
        }
        Expression::ArrayExpression(a) => {
            for el in &a.elements {
                if let Some(e) = el.as_expression() { sv_scan_expr_with_context(e, candidates, known_scalars, dis); }
            }
        }
        Expression::ObjectExpression(o) => {
            use oxc_ast::ast::ObjectPropertyKind;
            for prop in &o.properties {
                if let ObjectPropertyKind::ObjectProperty(p) = prop {
                    sv_scan_expr_with_context(&p.value, candidates, known_scalars, dis);
                }
            }
        }
        Expression::TemplateLiteral(t) => {
            for e in &t.expressions { sv_scan_expr_with_context(e, candidates, known_scalars, dis); }
        }
        Expression::NewExpression(n) => {
            for arg in &n.arguments {
                if let Some(e) = arg.as_expression() { sv_scan_expr_with_context(e, candidates, known_scalars, dis); }
            }
        }
        Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_) => {}
        _ => {}
    }
}

// ── Escape analysis for arena allocation ─────────────────────────────────────

/// Identify `const x = {}` or `const x = []` declarations in `stmts` where `x`
/// is provably local to the current function scope and does NOT escape to the heap
/// (i.e. is never returned, never passed as an argument to a non-whitelisted function,
/// never stored into another object/array, and never captured by a closure).
///
/// Variables in the returned set are safe to allocate on the fiber bump arena rather than
/// the global heap; the arena is freed at fiber exit so no ARC overhead is paid.
pub(crate) fn compute_non_escaping_allocs(stmts: &[oxc_ast::ast::Statement<'_>]) -> NameSet {
    let mut candidates: NameSet = NameSet::new();
    let mut escaped: NameSet = NameSet::new();

    // Pass 1: collect candidates — `const x = {}` or `const x = []`
    for stmt in stmts {
        nea_collect_candidates(stmt, &mut candidates);
    }
    if candidates.is_empty() {
        return candidates;
    }
    // Pass 2: scan for escaping uses, without recursing into nested function bodies.
    for stmt in stmts {
        nea_scan_stmt(stmt, &candidates, &mut escaped);
    }

    candidates.retain(|v| !escaped.contains(v));
    candidates
}

fn nea_collect_candidates<'a>(
    stmt: &'a oxc_ast::ast::Statement<'a>,
    cands: &mut NameSet,
) {
    use oxc_ast::ast::{Statement, VariableDeclarationKind};
    match stmt {
        Statement::VariableDeclaration(vd) => {
            if vd.kind == VariableDeclarationKind::Const {
                for decl in &vd.declarations {
                    if let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &decl.id {
                        if let Some(init) = &decl.init {
                            if matches!(init, oxc_ast::ast::Expression::ObjectExpression(_)
                                           | oxc_ast::ast::Expression::ArrayExpression(_)) {
                                cands.insert(id.name.to_string());
                            }
                        }
                    }
                }
            }
        }
        Statement::BlockStatement(b) => {
            for s in &b.body { nea_collect_candidates(s, cands); }
        }
        Statement::IfStatement(s) => {
            nea_collect_candidates(&s.consequent, cands);
            if let Some(alt) = &s.alternate { nea_collect_candidates(alt, cands); }
        }
        Statement::WhileStatement(s) => nea_collect_candidates(&s.body, cands),
        Statement::DoWhileStatement(s) => nea_collect_candidates(&s.body, cands),
        Statement::ForStatement(s) => nea_collect_candidates(&s.body, cands),
        Statement::LabeledStatement(s) => nea_collect_candidates(&s.body, cands),
        // Don't recurse into function/arrow — those have their own scope
        _ => {}
    }
}

/// Scan a statement for escaping uses of variables in `cands`.
/// Does NOT recurse into nested function/arrow bodies.
fn nea_scan_stmt<'a>(
    stmt: &'a oxc_ast::ast::Statement<'a>,
    cands: &NameSet,
    escaped: &mut NameSet,
) {
    use oxc_ast::ast::{Statement, ForStatementLeft};
    match stmt {
        Statement::ExpressionStatement(es) => nea_scan_expr(&es.expression, cands, escaped),
        Statement::ReturnStatement(r) => {
            if let Some(e) = &r.argument {
                // Returning a candidate variable = escape
                if let oxc_ast::ast::Expression::Identifier(id) = e {
                    if cands.contains(id.name.as_str()) {
                        escaped.insert(id.name.to_string());
                    }
                }
                nea_scan_expr(e, cands, escaped);
            }
        }
        Statement::ThrowStatement(s) => nea_scan_expr(&s.argument, cands, escaped),
        Statement::VariableDeclaration(vd) => {
            for decl in &vd.declarations {
                if let Some(init) = &decl.init {
                    // `const y = x` where x is a candidate → y is an alias → x escapes via y
                    if let oxc_ast::ast::BindingPattern::BindingIdentifier(_) = &decl.id {
                        if let oxc_ast::ast::Expression::Identifier(id) = init {
                            if cands.contains(id.name.as_str()) {
                                escaped.insert(id.name.to_string());
                            }
                        }
                    }
                    nea_scan_expr(init, cands, escaped);
                }
            }
        }
        Statement::IfStatement(s) => {
            nea_scan_expr(&s.test, cands, escaped);
            nea_scan_stmt(&s.consequent, cands, escaped);
            if let Some(alt) = &s.alternate { nea_scan_stmt(alt, cands, escaped); }
        }
        Statement::BlockStatement(b) => {
            for s in &b.body { nea_scan_stmt(s, cands, escaped); }
        }
        Statement::WhileStatement(s) => {
            nea_scan_expr(&s.test, cands, escaped);
            nea_scan_stmt(&s.body, cands, escaped);
        }
        Statement::DoWhileStatement(s) => {
            nea_scan_expr(&s.test, cands, escaped);
            nea_scan_stmt(&s.body, cands, escaped);
        }
        Statement::ForStatement(s) => {
            if let Some(init) = &s.init {
                if let Some(e) = init.as_expression() { nea_scan_expr(e, cands, escaped); }
            }
            if let Some(test) = &s.test { nea_scan_expr(test, cands, escaped); }
            if let Some(upd) = &s.update { nea_scan_expr(upd, cands, escaped); }
            nea_scan_stmt(&s.body, cands, escaped);
        }
        Statement::ForOfStatement(s) => {
            nea_scan_expr(&s.right, cands, escaped);
            nea_scan_stmt(&s.body, cands, escaped);
        }
        Statement::ForInStatement(s) => {
            nea_scan_expr(&s.right, cands, escaped);
            nea_scan_stmt(&s.body, cands, escaped);
        }
        Statement::SwitchStatement(s) => {
            nea_scan_expr(&s.discriminant, cands, escaped);
            for case in &s.cases {
                if let Some(test) = &case.test { nea_scan_expr(test, cands, escaped); }
                for cs in &case.consequent { nea_scan_stmt(cs, cands, escaped); }
            }
        }
        Statement::TryStatement(s) => {
            for cs in &s.block.body { nea_scan_stmt(cs, cands, escaped); }
            if let Some(h) = &s.handler {
                for cs in &h.body.body { nea_scan_stmt(cs, cands, escaped); }
            }
            if let Some(fin) = &s.finalizer {
                for cs in &fin.body { nea_scan_stmt(cs, cands, escaped); }
            }
        }
        Statement::LabeledStatement(s) => nea_scan_stmt(&s.body, cands, escaped),
        // Function declarations: don't recurse, but do check closure captures
        Statement::FunctionDeclaration(f) => {
            if let Some(body) = &f.body {
                for cs in &body.statements { nea_scan_stmt_closure(cs, cands, escaped); }
            }
        }
        _ => {}
    }
}

/// Check whether a candidate variable name appears ANYWHERE inside a closure body.
/// If it does, the variable escapes (it is captured by the closure's env array).
fn nea_scan_stmt_closure<'a>(
    stmt: &'a oxc_ast::ast::Statement<'a>,
    cands: &NameSet,
    escaped: &mut NameSet,
) {
    use oxc_ast::ast::Statement;
    match stmt {
        Statement::ExpressionStatement(es) => nea_escape_if_candidate(&es.expression, cands, escaped),
        Statement::ReturnStatement(r) => {
            if let Some(e) = &r.argument { nea_escape_if_candidate(e, cands, escaped); }
        }
        Statement::VariableDeclaration(vd) => {
            for decl in &vd.declarations {
                if let Some(init) = &decl.init { nea_escape_if_candidate(init, cands, escaped); }
            }
        }
        Statement::BlockStatement(b) => {
            for s in &b.body { nea_scan_stmt_closure(s, cands, escaped); }
        }
        Statement::IfStatement(s) => {
            nea_escape_if_candidate(&s.test, cands, escaped);
            nea_scan_stmt_closure(&s.consequent, cands, escaped);
            if let Some(alt) = &s.alternate { nea_scan_stmt_closure(alt, cands, escaped); }
        }
        Statement::WhileStatement(s) => {
            nea_escape_if_candidate(&s.test, cands, escaped);
            nea_scan_stmt_closure(&s.body, cands, escaped);
        }
        Statement::ForStatement(s) => {
            if let Some(upd) = &s.update { nea_escape_if_candidate(upd, cands, escaped); }
            nea_scan_stmt_closure(&s.body, cands, escaped);
        }
        _ => {}
    }
}

/// Mark any candidate identifier that appears anywhere in `expr` as escaped.
fn nea_escape_if_candidate(expr: &oxc_ast::ast::Expression<'_>, cands: &NameSet, escaped: &mut NameSet) {
    use oxc_ast::ast::Expression;
    match expr {
        Expression::Identifier(id) => {
            if cands.contains(id.name.as_str()) {
                escaped.insert(id.name.to_string());
            }
        }
        Expression::StaticMemberExpression(m) => nea_escape_if_candidate(&m.object, cands, escaped),
        Expression::ComputedMemberExpression(m) => {
            nea_escape_if_candidate(&m.object, cands, escaped);
            nea_escape_if_candidate(&m.expression, cands, escaped);
        }
        Expression::CallExpression(c) => {
            nea_escape_if_candidate(&c.callee, cands, escaped);
            for arg in &c.arguments {
                if let Some(e) = arg.as_expression() { nea_escape_if_candidate(e, cands, escaped); }
            }
        }
        Expression::AssignmentExpression(a) => {
            nea_escape_if_candidate(&a.right, cands, escaped);
        }
        Expression::BinaryExpression(b) => {
            nea_escape_if_candidate(&b.left, cands, escaped);
            nea_escape_if_candidate(&b.right, cands, escaped);
        }
        Expression::LogicalExpression(l) => {
            nea_escape_if_candidate(&l.left, cands, escaped);
            nea_escape_if_candidate(&l.right, cands, escaped);
        }
        Expression::ConditionalExpression(c) => {
            nea_escape_if_candidate(&c.test, cands, escaped);
            nea_escape_if_candidate(&c.consequent, cands, escaped);
            nea_escape_if_candidate(&c.alternate, cands, escaped);
        }
        Expression::UnaryExpression(u) => nea_escape_if_candidate(&u.argument, cands, escaped),
        Expression::TemplateLiteral(t) => {
            for e in &t.expressions { nea_escape_if_candidate(e, cands, escaped); }
        }
        Expression::ArrayExpression(a) => {
            for el in &a.elements {
                if let Some(e) = el.as_expression() { nea_escape_if_candidate(e, cands, escaped); }
            }
        }
        Expression::ObjectExpression(o) => {
            use oxc_ast::ast::ObjectPropertyKind;
            for prop in &o.properties {
                if let ObjectPropertyKind::ObjectProperty(p) = prop {
                    nea_escape_if_candidate(&p.value, cands, escaped);
                }
            }
        }
        _ => {}
    }
}

/// Scan an expression for escaping uses of candidates.
/// A "safe" use is:
///   - `x.prop` / `x[i]` (member read with x as object)
///   - `x.prop = val` / `x[i] = val` (member write TO x)
///   - `x.method(args)` (method call with x as receiver `this`)
///   - `x instanceof Foo`, `typeof x`, `!x`, `x ? ... : ...` (boolean/type tests)
///   - `JSON.stringify(x)`, `console.*(..., x, ...)` (whitelisted callee)
///   - spread of x: `{...x}` (reads properties, doesn't store x)
///
/// Everything else is considered escaping.
fn nea_scan_expr<'a>(
    expr: &'a oxc_ast::ast::Expression<'a>,
    cands: &NameSet,
    escaped: &mut NameSet,
) {
    use oxc_ast::ast::Expression;
    match expr {
        Expression::Identifier(_) => {
            // Bare identifier use — check context in the callers (e.g. ReturnStatement)
        }
        Expression::AssignmentExpression(a) => {
            nea_scan_expr(&a.right, cands, escaped);
            // RHS: if a candidate appears, it might be stored into the LHS target.
            // Check if x is directly on the RHS (x = ...) → covered by VariableDeclaration scan.
            // If lhs is a member (obj.prop = x) where x is a candidate → escape.
            if matches!(&a.left,
                oxc_ast::ast::AssignmentTarget::StaticMemberExpression(_)
                | oxc_ast::ast::AssignmentTarget::ComputedMemberExpression(_))
            {
                // RHS value is being stored into a heap object — if RHS is a candidate, escape it.
                if let Expression::Identifier(id) = &a.right {
                    if cands.contains(id.name.as_str()) {
                        escaped.insert(id.name.to_string());
                    }
                }
            }
            // Also escape if candidate is assigned to a simple identifier (alias).
            if let oxc_ast::ast::AssignmentTarget::AssignmentTargetIdentifier(_) = &a.left {
                if let Expression::Identifier(id) = &a.right {
                    if cands.contains(id.name.as_str()) {
                        escaped.insert(id.name.to_string());
                    }
                }
            }
            // Also scan the full RHS for nested escaping patterns.
            nea_scan_rhs_for_candidates(&a.right, cands, escaped);
        }
        Expression::CallExpression(c) => {
            // Check if this is a safe whitelisted call (JSON.stringify, console.*)
            if is_whitelisted_call(c) {
                // Safe — arguments may be candidates, that's OK
                for arg in &c.arguments {
                    if let Some(e) = arg.as_expression() { nea_scan_expr(e, cands, escaped); }
                }
            } else if let oxc_ast::ast::Expression::StaticMemberExpression(m) = &c.callee {
                // Method call: x.method(args) — x is the receiver, args might escape
                // x (the receiver) is SAFE (method acts on x), but args are potentially unsafe
                nea_scan_expr(&m.object, cands, escaped);
                for arg in &c.arguments {
                    if let Some(e) = arg.as_expression() {
                        // Argument to a method: if it's a candidate, it might be stored
                        if let Expression::Identifier(id) = e {
                            if cands.contains(id.name.as_str()) {
                                escaped.insert(id.name.to_string());
                            }
                        }
                        nea_scan_expr(e, cands, escaped);
                    }
                }
            } else {
                // Unknown call — all candidate arguments escape
                nea_scan_expr(&c.callee, cands, escaped);
                for arg in &c.arguments {
                    if let Some(e) = arg.as_expression() {
                        if let Expression::Identifier(id) = e {
                            if cands.contains(id.name.as_str()) {
                                escaped.insert(id.name.to_string());
                            }
                        }
                        nea_scan_expr(e, cands, escaped);
                    }
                }
            }
        }
        Expression::NewExpression(n) => {
            // Constructor args might store the candidate
            for arg in &n.arguments {
                if let Some(e) = arg.as_expression() {
                    if let Expression::Identifier(id) = e {
                        if cands.contains(id.name.as_str()) {
                            escaped.insert(id.name.to_string());
                        }
                    }
                    nea_scan_expr(e, cands, escaped);
                }
            }
        }
        Expression::StaticMemberExpression(m) => nea_scan_expr(&m.object, cands, escaped),
        Expression::ComputedMemberExpression(m) => {
            nea_scan_expr(&m.object, cands, escaped);
            nea_scan_expr(&m.expression, cands, escaped);
        }
        Expression::BinaryExpression(b) => {
            nea_scan_expr(&b.left, cands, escaped);
            nea_scan_expr(&b.right, cands, escaped);
        }
        Expression::LogicalExpression(l) => {
            nea_scan_expr(&l.left, cands, escaped);
            nea_scan_expr(&l.right, cands, escaped);
        }
        Expression::UnaryExpression(u) => nea_scan_expr(&u.argument, cands, escaped),
        Expression::ConditionalExpression(c) => {
            nea_scan_expr(&c.test, cands, escaped);
            nea_scan_expr(&c.consequent, cands, escaped);
            nea_scan_expr(&c.alternate, cands, escaped);
        }
        Expression::SequenceExpression(s) => {
            for e in &s.expressions { nea_scan_expr(e, cands, escaped); }
        }
        Expression::TemplateLiteral(t) => {
            for e in &t.expressions { nea_scan_expr(e, cands, escaped); }
        }
        Expression::ObjectExpression(o) => {
            // Values stored in object literals escape (unless the containing object is also arena).
            // Conservative: mark any candidate appearing as a property value as escaped.
            use oxc_ast::ast::ObjectPropertyKind;
            for prop in &o.properties {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        if let Expression::Identifier(id) = &p.value {
                            if cands.contains(id.name.as_str()) {
                                escaped.insert(id.name.to_string());
                            }
                        }
                        nea_scan_expr(&p.value, cands, escaped);
                    }
                    ObjectPropertyKind::SpreadProperty(sp) => {
                        // `{...x}` — reads from x (safe), x doesn't escape
                        nea_scan_expr(&sp.argument, cands, escaped);
                    }
                }
            }
        }
        Expression::ArrayExpression(a) => {
            // Elements stored in array literals escape.
            for el in &a.elements {
                use oxc_ast::ast::ArrayExpressionElement;
                match el {
                    ArrayExpressionElement::SpreadElement(sp) => {
                        // `[...x]` — iterates x (safe), x doesn't escape as an element
                        nea_scan_expr(&sp.argument, cands, escaped);
                    }
                    _ => {
                        if let Some(e) = el.as_expression() {
                            if let Expression::Identifier(id) = e {
                                if cands.contains(id.name.as_str()) {
                                    escaped.insert(id.name.to_string());
                                }
                            }
                            nea_scan_expr(e, cands, escaped);
                        }
                    }
                }
            }
        }
        Expression::ArrowFunctionExpression(f) => {
            // Closure body: any candidate referenced inside is a capture → escape
            for s in f.body.statements.iter() {
                nea_scan_stmt_closure(s, cands, escaped);
            }
        }
        Expression::FunctionExpression(f) => {
            if let Some(body) = &f.body {
                for s in &body.statements { nea_scan_stmt_closure(s, cands, escaped); }
            }
        }
        Expression::TSAsExpression(e) => nea_scan_expr(&e.expression, cands, escaped),
        Expression::TSNonNullExpression(e) => nea_scan_expr(&e.expression, cands, escaped),
        Expression::TSTypeAssertion(e) => nea_scan_expr(&e.expression, cands, escaped),
        Expression::TSInstantiationExpression(e) => nea_scan_expr(&e.expression, cands, escaped),
        Expression::ParenthesizedExpression(e) => nea_scan_expr(&e.expression, cands, escaped),
        Expression::AwaitExpression(e) => {
            // await expr — the expr value (e.g. a Promise) doesn't cause escape of candidates
            // unless expr itself contains the candidate.
            nea_scan_expr(&e.argument, cands, escaped);
        }
        _ => {}
    }
}

/// Scan RHS deeply for any candidate identifier — used when we know the context is
/// "being stored" (e.g. stored into an object property or array), so any candidate
/// appearing anywhere in the sub-expression should be escaped.
fn nea_scan_rhs_for_candidates(expr: &oxc_ast::ast::Expression<'_>, cands: &NameSet, escaped: &mut NameSet) {
    nea_escape_if_candidate(expr, cands, escaped);
}

/// Returns true if this call expression is whitelisted as a "non-storing" callee.
/// Specifically: JSON.stringify(x) and console.*(x) are known to read x (serialize/print)
/// without storing a long-term reference to x.
fn is_whitelisted_call(call: &oxc_ast::ast::CallExpression<'_>) -> bool {
    use oxc_ast::ast::Expression;
    match &call.callee {
        Expression::StaticMemberExpression(m) => {
            // console.log / console.error / console.warn / console.debug / console.info
            if let Expression::Identifier(obj) = &m.object {
                if obj.name == "console" {
                    return true;
                }
                // JSON.stringify
                if obj.name == "JSON" && m.property.name == "stringify" {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// Like `expr_is_definitely_scalar` but also accepts identifiers in `known_scalars`.
fn expr_result_is_scalar_with_context(
    expr: &oxc_ast::ast::Expression<'_>,
    known_scalars: &NameSet,
) -> bool {
    use oxc_ast::ast::Expression;
    match expr {
        Expression::Identifier(id) => {
            let n = id.name.as_str();
            n == "undefined" || n == "NaN" || n == "Infinity" || known_scalars.contains(n)
        }
        Expression::TSAsExpression(e) => expr_result_is_scalar_with_context(&e.expression, known_scalars),
        Expression::TSNonNullExpression(e) => expr_result_is_scalar_with_context(&e.expression, known_scalars),
        Expression::TSTypeAssertion(e) => expr_result_is_scalar_with_context(&e.expression, known_scalars),
        Expression::TSInstantiationExpression(e) => expr_result_is_scalar_with_context(&e.expression, known_scalars),
        Expression::ParenthesizedExpression(e) => expr_result_is_scalar_with_context(&e.expression, known_scalars),
        _ => crate::lowering::expr_is_definitely_scalar(expr),
    }
}
