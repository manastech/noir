use crate::hir_def::types::Type;
pub use noirc_errors::debug_info::{Types, VariableTypes, Variables};
use noirc_printable_type::PrintableType;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct DebugTypes {
    variables: HashMap<u32, (String, u32)>, // var_id => (var_name, type_id)
    types: HashMap<PrintableType, u32>,
    id_to_type: HashMap<u32, PrintableType>,
    next_type_id: u32,
}

impl DebugTypes {
    pub fn insert_var(&mut self, var_id: u32, var_name: &str, var_type: Type) {
        let ptype: PrintableType = var_type.into();
        let type_id = self.types.get(&ptype).cloned().unwrap_or_else(|| {
            let type_id = self.next_type_id;
            self.next_type_id += 1;
            self.types.insert(ptype.clone(), type_id);
            self.id_to_type.insert(type_id, ptype);
            type_id
        });
        self.variables.insert(var_id, (var_name.to_string(), type_id));
    }

    pub fn get_type<'a>(&'a self, var_id: u32) -> Option<&'a PrintableType> {
        self.variables.get(&var_id).and_then(|(_, type_id)| self.id_to_type.get(&type_id))
    }
}

impl From<DebugTypes> for VariableTypes {
    fn from(val: DebugTypes) -> Self {
        (
            val.variables.into_iter().collect(),
            val.types.into_iter().map(|(ptype, type_id)| (type_id, ptype)).collect(),
        )
    }
}
