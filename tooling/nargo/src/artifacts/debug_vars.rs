use acvm::brillig_vm::brillig::Value;
use noirc_printable_type::{decode_value, PrintableType, PrintableValue};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default, Clone)]
pub struct DebugVars {
    id_to_name: HashMap<u32, String>,
    frames: Vec<(u32, HashMap<u32, PrintableValue>)>,
    id_to_type: HashMap<u32, u32>,
    types: HashMap<u32, PrintableType>,
}

impl DebugVars {
    pub fn new(vars: &HashMap<u32, String>) -> Self {
        Self { id_to_name: vars.clone(), ..Self::default() }
    }

    pub fn get_variables(
        &self,
    ) -> Vec<(&str, Vec<&str>, Vec<(&str, &PrintableValue, &PrintableType)>)> {
        println!("get_variables");
        println!("id_to_name: {:#?}", self.id_to_name);
        println!("frames: {:#?}", self.frames);
        println!("id_to_type: {:#?}", self.id_to_type);
        println!("types: {:#?}", self.types);

        self.frames
            .iter()
            .map(|(fn_id, frame)| {
                // println!("frame: {:#?}", frame);
                // println!("fn_id: {:#?}", fn_id);

                let fn_type_id =
                    self.id_to_type.get(fn_id).expect("failed to find type for fn_id={fn_id}");

                // println!("fn_type_id: {:#?}", fn_type_id);

                let fn_type = self
                    .types
                    .get(fn_type_id)
                    .expect(&format!("failed to get function type for fn_type_id={fn_type_id}"));
                let PrintableType::Function { name, arguments, .. } = fn_type
                else { panic!("unexpected function type {fn_type:?}") };                

                let params: Vec<&str> =
                    arguments.iter().map(|(var_name, _)| var_name.as_str()).collect();
                let vars: Vec<(&str, &PrintableValue, &PrintableType)> =
                    frame.iter().filter_map(|(var_id, var_value)| {
                        self.lookup_var(*var_id).map(|(name, typ)| {
                            (name, var_value, typ)
                        })
                    })
                    .collect();

                // println!("name: {:#?}", name);
                // println!("params: {:#?}", params);
                // println!("vars: {:#?}", vars);

                (name.as_str(), params, vars)
            })
            .collect()
    }

    fn lookup_var(&self, var_id: u32) -> Option<(&str, &PrintableType)> {
        // println!("lookup var_id: {:#?}", var_id);
        // println!("in id_to_name: {:#?}", self.id_to_name);
        self.id_to_name
            .get(&var_id)            
            .and_then(|name| {
                self.id_to_type.get(&var_id).map(|type_id| (name, type_id))
            })
            .and_then(|(name, type_id)| {
                self.types.get(type_id).map(|ptype| (name.as_str(), ptype))
            })
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
        let type_id = self.id_to_type.get(&var_id).unwrap();
        let ptype = self.types.get(type_id).unwrap();

        self.frames.last_mut()
            .expect("unexpected empty stack frames").1
            .insert(var_id, decode_value(&mut values.iter().map(|v| v.to_field()), ptype));
    }

    pub fn assign_field(&mut self, var_id: u32, indexes: Vec<u32>, values: &[Value]) {
        let mut current_frame = &mut self
            .frames.last_mut()
            .expect("unexpected empty stack frames").1;

        let mut cursor: &mut PrintableValue = current_frame            
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
                    if let Some(len) = length {
                        if *index as u64 >= *len {
                            panic!("unexpected field index past array length")
                        }
                        if *len != array.len() as u64 {
                            panic!("type/array length mismatch")
                        }
                    }
                    (array.get_mut(*index as usize).unwrap(), &*Box::leak(typ.clone()))
                }
                (
                    PrintableValue::Struct(field_map),
                    PrintableType::Struct { name: _name, fields },
                ) => {
                    if *index as usize >= fields.len() {
                        panic!("unexpected field index past struct field length")
                    }
                    let (key, typ) = fields.get(*index as usize).unwrap();
                    (field_map.get_mut(key).unwrap(), typ)
                }
                (PrintableValue::Vec(array), PrintableType::Tuple { types }) => {
                    if *index >= types.len() as u32 {
                        panic!(
                            "unexpected field index ({index}) past tuple length ({})",
                            types.len()
                        );
                    }
                    if types.len() != array.len() {
                        panic!("type/array length mismatch")
                    }
                    let typ = types.get(*index as usize).unwrap();
                    (array.get_mut(*index as usize).unwrap(), typ)
                }
                _ => {
                    panic!("unexpected assign field of {cursor_type:?} type");
                }
            };
        }
        *cursor = decode_value(&mut values.iter().map(|v| v.to_field()), cursor_type);
        
        //TODO: I think this is not necessary because current_frame and
        // cursor are already mutably borrowed
        //current_frame.insert(var_id, *cursor);
    }

    pub fn assign_deref(&mut self, _var_id: u32, _values: &[Value]) {
        // TODO
        unimplemented![]
    }

    pub fn push_fn(&mut self, fn_id: u32) {
        self.frames.push((fn_id, HashMap::default()));
    }

    pub fn pop_fn(&mut self) {
        self.frames.pop();
    }

    pub fn get(&mut self, var_id: u32) -> Option<&PrintableValue> {
        self.frames.last_mut()
            .expect("unexpected empty stack frames")
            .1.get(&var_id)
    }

    pub fn get_type(&self, var_id: u32) -> Option<&PrintableType> {
        self.id_to_type.get(&var_id).and_then(|type_id| self.types.get(type_id))
    }

    pub fn drop(&mut self, var_id: u32) {
        self.frames.last_mut().expect("unexpected empty stack frames").1.remove(&var_id);
    }
}
