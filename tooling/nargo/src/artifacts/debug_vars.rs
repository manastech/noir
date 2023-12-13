use acvm::brillig_vm::brillig::Value;
use noirc_printable_type::{PrintableType, PrintableValue};
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
        Self { id_to_name: vars.clone(), ..Self::default() }
    }

    pub fn get_variables(&self) -> Vec<(&str, &PrintableValue, &PrintableType)> {
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
        });
    }

    pub fn insert_types(&mut self, types: &HashMap<u32, PrintableType>) {
        types.iter().for_each(|(type_id, ptype)| {
            self.types.insert(*type_id, ptype.clone());
        });
    }

    pub fn assign(&mut self, var_id: u32, values: &[Value]) {
        self.active.insert(var_id);
        // TODO: assign values as PrintableValue
        let type_id = self.id_to_type.get(&var_id).unwrap();
        let ptype = self.types.get(type_id).unwrap();
        self.id_to_value.insert(var_id, create_value(ptype, values));
    }

    pub fn assign_field(&mut self, var_id: u32, indexes: Vec<u32>, values: &[Value]) {
        let mut cursor: &mut PrintableValue = self
            .id_to_value
            .get_mut(&var_id)
            .unwrap_or_else(|| panic!("value unavailable for var_id {var_id}"));
        let cursor_type_id = self
            .id_to_type
            .get(&var_id)
            .unwrap_or_else(|| panic!("type id unavailable for var_id {var_id}"));
        let mut cursor_type = self
            .types
            .get(cursor_type_id)
            .unwrap_or_else(|| panic!("type unavailable for type id {cursor_type_id}"));
        for index in indexes.iter() {
            (cursor, cursor_type) = match (cursor, cursor_type) {
                (PrintableValue::Vec(array), PrintableType::Array { length, typ }) => {
                    if *index as u64 >= *length {
                        panic!("unexpected field index past array length")
                    }
                    if *length != array.len() as u64 {
                        panic!("type/array length mismatch")
                    }
                    (array.get_mut(*index as usize).unwrap(), &*Box::leak(typ.clone()))
                }
                (PrintableValue::Struct(field_map), PrintableType::Struct { name: _, fields }) => {
                    if *index as usize >= fields.len() {
                        panic!("unexpected field index past struct field length")
                    }
                    let (key, typ) = fields.get(*index as usize).unwrap();
                    (field_map.get_mut(key).unwrap(), typ)
                }
                _ => {
                    panic!("unexpected assign field of {cursor_type:?} type");
                }
            };
        }
        *cursor = create_value(cursor_type, values);
        self.active.insert(var_id);
    }

    pub fn assign_deref(&mut self, _var_id: u32, _values: &[Value]) {
        // TODO
        unimplemented![]
    }

    pub fn get(&mut self, var_id: u32) -> Option<&PrintableValue> {
        self.id_to_value.get(&var_id)
    }

    pub fn drop(&mut self, var_id: u32) {
        self.active.remove(&var_id);
    }
}

// TODO: put this in a From or Into impl
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
        }
        PrintableType::Array { length, typ } => {
            if values.len() as u64 != *length {
                panic!["array type length ({}) != value length ({})", length, values.len()];
            }
            PrintableValue::Vec(values.iter().map(|v| create_value(typ, &[*v])).collect())
        }
        PrintableType::Struct { name: _name, fields } => PrintableValue::Struct(
            fields
                .iter()
                .zip(values.iter())
                .map(|((key, stype), v)| (key.clone(), create_value(stype, &[*v])))
                .collect(),
        ),
        PrintableType::String { length: _length } => PrintableValue::String(
            String::from_utf8(values.iter().map(|v| v.to_field().to_u128() as u8).collect())
                .expect("string utf8 decode failed"),
        ),
    }
}
