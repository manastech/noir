struct Foo {
    field: Field,
    array: [Field; 3],
    slice: [Field],
}

fn main(
    dynamic_one: Field, // == 1
) {
    let foo_dynamic_array = Foo { field: 0, array: [dynamic_one, 2, 3], slice: &[] };
    assert_constant(foo_dynamic_array);
}
