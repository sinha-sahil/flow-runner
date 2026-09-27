//! The top-level declarations: every name declared once, params with
//! literal defaults, helpers as temple functions, and annotations.

use super::scope::Scope;
use super::{Compiler, Declarations, Kind};
use crate::common::diagnostic::Diagnostic;
use crate::common::error::{already_declared, twice};
use crate::flow::{Annotation, Helper, Let, Param};
use crate::syntax::ast::{self, Identifier, LetDeclaration};
use crate::syntax::names::name_problem;
use std::collections::HashSet;
use temple_dsl::{Functions, Span, Value};

impl Compiler<'_> {
    /// Every param, let, helper and node, which share one namespace.
    pub(super) fn declare_globals(&mut self) -> Declarations {
        let ast = self.ast;
        let mut globals = Declarations::new();
        for param in &ast.params {
            self.declare(&mut globals, &param.name, Kind::Param);
        }
        for binding in &ast.lets {
            let kind = match binding.params {
                Some(_) => Kind::Helper,
                None => Kind::Let,
            };
            self.declare(&mut globals, &binding.name, kind);
        }
        for node in &ast.nodes {
            self.declare(&mut globals, &node.name, Kind::Node);
        }
        globals
    }

    fn declare(&mut self, declarations: &mut Declarations, identifier: &Identifier, kind: Kind) {
        self.check_name(identifier);
        if let Some(existing) = declarations.get(&identifier.name) {
            self.push(
                Diagnostic::error(
                    identifier.span,
                    already_declared(&identifier.name, existing.noun()),
                )
                .with_help("every name in a flow must be unique"),
            );
        } else {
            declarations.insert(identifier.name.clone(), kind);
        }
    }

    pub(super) fn check_name(&mut self, identifier: &Identifier) {
        if let Some(problem) = name_problem(&identifier.name) {
            self.error(identifier.span, problem);
        }
    }

    /// A parameter of a helper or node, which must be new to the flow and
    /// to the names in `locals`, where it is added.
    pub(super) fn check_parameter(
        &mut self,
        param: &Identifier,
        globals: &Declarations,
        locals: &mut Declarations,
    ) {
        self.check_name(param);
        if let Some(existing) = globals.get(&param.name) {
            let declared = already_declared(&param.name, existing.noun());
            self.error(
                param.span,
                format!("{declared}; parameters need their own names"),
            );
        }
        if locals.insert(param.name.clone(), Kind::Parameter).is_some() {
            self.error(param.span, twice("parameter", &param.name));
        }
    }

    pub(super) fn params(&mut self) -> Vec<Param> {
        let mut params = Vec::new();
        for param in &self.ast.params {
            let annotations = self.annotations(&param.annotations);
            if let Some(default) = self.literal(&param.default, "a param default") {
                params.push(Param {
                    name: param.name.name.to_string(),
                    default,
                    annotations,
                });
            }
        }
        params
    }

    /// The flow lets, compiled. Each sees the params, the helpers and the lets
    /// before it.
    pub(super) fn flow_lets(&mut self, scope: &mut Scope) -> Vec<Let> {
        let mut lets = Vec::new();
        for binding in self
            .ast
            .lets
            .iter()
            .filter(|binding| binding.params.is_none())
        {
            if let Some(expression) = self.expression(&binding.expression, scope) {
                lets.push(Let {
                    name: binding.name.name.clone(),
                    expression,
                });
            }
            scope.add_variable(&binding.name.name);
        }
        lets
    }

    /// The helpers, compiled, and as temple functions. Helpers see the
    /// params and each other. When one fails, the functions are `None`:
    /// building them would report its callers too.
    pub(super) fn helpers(
        &mut self,
        globals: &Declarations,
        scope: &mut Scope,
    ) -> (Vec<Helper>, Option<Functions>) {
        let ast = self.ast;
        let declared: Vec<_> = ast
            .lets
            .iter()
            .filter(|binding| binding.params.is_some())
            .collect();
        for helper in &declared {
            let arity = helper.params.as_ref().map_or(0, Vec::len);
            scope.add_function(&helper.name.name, arity);
        }
        for param in &ast.params {
            scope.add_variable(&param.name.name);
        }
        let helpers: Vec<_> = declared
            .iter()
            .filter_map(|helper| self.helper(helper, globals, scope))
            .collect();
        if helpers.len() != declared.len() {
            return (helpers, None);
        }
        let functions = self.functions(&helpers);
        (helpers, functions)
    }

    /// One helper, compiled. Its parameters are in scope for its body only.
    fn helper(
        &mut self,
        declaration: &LetDeclaration,
        globals: &Declarations,
        scope: &mut Scope,
    ) -> Option<Helper> {
        let params = declaration.params.as_deref().unwrap_or_default();
        let mut locals = Declarations::new();
        for param in params {
            self.check_parameter(param, globals, &mut locals);
            scope.add_local(&param.name);
        }
        let body = self.expression(&declaration.expression, scope);
        scope.clear_locals();
        Some(Helper {
            name: declaration.name.name.clone(),
            params: params.iter().map(|param| param.name.clone()).collect(),
            body: body?,
        })
    }

    /// The helpers as temple functions, which also refuses recursion.
    fn functions(&mut self, helpers: &[Helper]) -> Option<Functions> {
        let definitions = helpers.iter().map(|helper| {
            (
                helper.name.clone(),
                helper.params.clone(),
                helper.body.clone(),
            )
        });
        match Functions::build(definitions) {
            Ok(functions) => Some(functions),
            Err(errors) => {
                self.temple_errors(&errors, Span::new(0, 0));
                None
            }
        }
    }

    pub(super) fn annotations(&mut self, annotations: &[ast::Annotation]) -> Vec<Annotation> {
        let mut compiled = Vec::new();
        let mut seen = HashSet::new();
        for annotation in annotations {
            if !seen.insert(&annotation.name.name) {
                self.error(
                    annotation.name.span,
                    twice("annotation", &format!("@{}", annotation.name.name)),
                );
                continue;
            }
            let value = match &annotation.value {
                None => Some(Value::Bool(true)),
                Some(parsed) => self.literal(parsed, "an annotation value"),
            };
            if let Some(value) = value {
                compiled.push(Annotation {
                    name: annotation.name.name.to_string(),
                    value,
                });
            }
        }
        compiled
    }
}
