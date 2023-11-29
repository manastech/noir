use noirc_printable_type::{PrintableType,PrintableValue};
use acvm::brillig_vm::brillig::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default, Clone)]
pub struct DebugVars {
    id_to_name: HashMap<u32, String>,
    active: HashSet<u32>,
    id_to_value: HashMap<u32, PrintableValue>, // TODO: something more sophisticated for lexical levels
    id_to_type: HashMap<u32, u32>,
    types: HashMap<u32, PrintableType>,
}

impl DebugVars {
    pub fn new(vars: &HashMap<u32, String>) -> Self {
        let mut debug_vars = Self::default();
        debug_vars.id_to_name = vars.clone();
        debug_vars
    }

    pub fn get_variables<'a>(&'a self) -> Vec<(&'a str, &'a PrintableValue, &'a PrintableType)> {
        self.active
            .iter()
            .filter_map(|var_id| {
                self.id_to_name
                    .get(var_id)
                    .and_then(|name| self.id_to_value.get(var_id).map(|value| (name, value)))
                    .and_then(|(name, value)| {
                        self.id_to_type.get(var_id).map(|type_id| (name, value, type_id))
                    })
                    .and_then(|(name, value, type_id)| {
                        self.types.get(type_id).map(|ptype| (name.as_str(), value, ptype))
                    })
            })
            .collect()
    }

    pub fn insert_variables(&mut self, vars: &HashMap<u32, (String, u32)>) {
        vars.iter().for_each(|(var_id, (var_name, var_type))| {
            self.id_to_name.insert(*var_id, var_name.clone());
            self.id_to_type.insert(*var_id, *var_type);
            println!["INSERT VARIABLE {var_name}:{var_id} => {var_type}"];
        });
    }

    pub fn insert_types(&mut self, types: &HashMap<u32, PrintableType>) {
        types.iter().for_each(|(type_id, ptype)| {
            self.types.insert(*type_id, ptype.clone());
        });
    }

    pub fn assign(&mut self, var_id: u32, values: &[Value]) {
        self.active.insert(var_id);
        println!["var_id={var_id:?}"];
        // TODO: assign values as PrintableValue
        let type_id = self.id_to_type.get(&var_id).unwrap();
        let ptype = self.types.get(&type_id).unwrap();
        self.id_to_value.insert(var_id, create_value(ptype, values));
    }

    pub fn assign_field(&mut self, var_id: u32, indexes: Vec<u32>, values: &[Value]) {
        let mut cursor: &mut PrintableValue = self.id_to_value.get_mut(&var_id)
            .expect(&format!["value unavailable for var_id {var_id}"]);
        let cursor_type_id = self.id_to_type.get(&var_id)
            .expect(&format!["type id unavailable for var_id {var_id}"]);
        let mut cursor_type = self.types.get(cursor_type_id)
            .expect(&format!["type unavailable for type id {cursor_type_id}"]);
        for index in indexes.iter() {
            (cursor, cursor_type) = match (cursor,cursor_type) {
                (PrintableValue::Vec(array), PrintableType::Array { length, typ }) => {
                    if *index as u64 >= *length { panic!("unexpected field index past array length") }
                    if *length != array.len() as u64 { panic!("type/array length mismatch") }
                    (array.get_mut(*index as usize).unwrap(), &*Box::leak(typ.clone()))
                },
                (PrintableValue::Struct(field_map), PrintableType::Struct { name, fields }) => {
                    if *index as usize >= fields.len() { panic!("unexpected field index past struct field length") }
                    let (key,typ) = fields.get(*index as usize).unwrap();
                    (field_map.get_mut(key).unwrap(), typ)
                },
                _ => {
                    panic!("unexpected assign field of {cursor_type:?} type");
                },
            };
        }
        assign_values(cursor, values);
        self.active.insert(var_id);
    }

    pub fn assign_deref(&mut self, _var_id: u32, _values: &[Value]) {
        // TODO
        unimplemented![]
    }

    pub fn get<'a>(&'a mut self, var_id: u32) -> Option<&'a PrintableValue> {
        self.id_to_value.get(&var_id)
    }

    pub fn drop(&mut self, var_id: u32) {
        self.active.remove(&var_id);
    }
}

fn create_value(ptype: &PrintableType, values: &[Value]) -> PrintableValue {
    match ptype {
        PrintableType::Field
        | PrintableType::SignedInteger { .. }
        | PrintableType::UnsignedInteger { .. }
        | PrintableType::Boolean => {
            if values.len() != 1 {
                panic!["unexpected number of values ({}) for field", values.len()];
            }
            PrintableValue::Field(values[0].to_field())
        },
        PrintableType::Array { length, typ } => {
            if values.len() as u64 != *length {
                panic!["array type length ({}) != value length ({})", length, values.len()];
            }
            PrintableValue::Vec(values.iter()
                .map(|v| create_value(typ, &[*v]))
                .collect()
            )
        },
        PrintableType::Struct { name: _name, fields: _fields } => {
            unimplemented![]
        },
        PrintableType::String { length: _length } => {
            unimplemented![]
        },
    }
}

fn assign_values(dst: &mut PrintableValue, values: &[Value]) {
    //unimplemented![]
}
