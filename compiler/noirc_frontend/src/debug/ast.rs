use noirc_errors::Location;
use chumsky::prelude::recursive;
use crate::parser::ParsedModule;
use crate::ast;

// debug struct
// assert values
// variable values
// function arguments
// program gets linearized so maybe show arguments as if that wasn't the case

fn walk_expr(expr: &mut ast::Expression) {
}

pub fn insert_debug_symbols(module: &mut ParsedModule) {
}
