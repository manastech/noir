use std::ops::{WrappingAdd, WrappingMul, WrappingSub};

//Basic tests for arrays
fn main(x: [u32; 5], y: [u32; 5], mut z: u32, t: u32) {
    let mut c = 2301;
    z = y[4];
    //Test 1:
    for i in 0..5 {
        c = z * z * y[i];
        z -= c;
    }
    assert(z == 0); //y[4]=0, so c and z are always 0
    //Test 2:
    c = 2301 as u32;
    for i in 0..5 {
        c = t + 2 as u32;
        c = z.wrapping_mul(z).wrapping_mul(x[i]);
        z = z.wrapping_add((x[i] * y[i]).wrapping_sub(c));
    }
    assert(z == 3814912846);
    //Test 3:
    c = 2300001 as u32;
    z = y[4];
    for i in 0..5 {
        z = z + x[i] * y[i];
        for _i in 0..3 {
            c = (i as u32).wrapping_sub(2 as u32);
            z = z.wrapping_mul(c);
        }
    }
    assert(z == 41472);
    //Test 4:
    z = y[4];
    for i in 0..3 {
        z += x[i] * y[i];
        for j in 0..2 {
            z += x[i + j] - y[i + j];
        }
    }
    assert(z == 11539);
    //Test 5:
    let cc = if z < 1 { x } else { y };
    assert(cc[0] == y[0]);
    // Test 6: for-each loops
    for y_elem in y {
        for x_elem in x {
            assert(x_elem != y_elem);
        }
    }
    // Test 7: Arrays of tuples/structs
    let mut tuple_array = [(1, 2), (3, 4), (5, 6)];
    tuple_array[1] = (7, 8);
    assert(tuple_array[1].1 == 8);
}
