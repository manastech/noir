use crate::{HeapArray, MemoryAddress, opcodes::HeapVector};
use serde::{Deserialize, Serialize};

/// These opcodes provide an equivalent of ACIR blackbox functions.
/// They are implemented as native functions in the VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "arb", derive(proptest_derive::Arbitrary))]
pub enum BlackBoxOp {
    /// Encrypts a message using AES128.
    AES128Encrypt {
        inputs: HeapVector,
        iv: HeapArray,
        key: HeapArray,
        outputs: HeapVector,
    },
    /// Calculates the Blake2s hash of the inputs.
    Blake2s {
        message: HeapVector,
        output: HeapArray,
    },
    /// Calculates the Blake3 hash of the inputs.
    Blake3 {
        message: HeapVector,
        output: HeapArray,
    },
    /// Keccak Permutation function of 1600 width
    Keccakf1600 {
        input: HeapArray,
        output: HeapArray,
    },
    /// Verifies a ECDSA signature over the secp256k1 curve.
    EcdsaSecp256k1 {
        hashed_msg: HeapVector,
        public_key_x: HeapArray,
        public_key_y: HeapArray,
        signature: HeapArray,
        result: MemoryAddress,
    },
    /// Verifies a ECDSA signature over the secp256r1 curve.
    EcdsaSecp256r1 {
        hashed_msg: HeapVector,
        public_key_x: HeapArray,
        public_key_y: HeapArray,
        signature: HeapArray,
        result: MemoryAddress,
    },

    /// Performs multi scalar multiplication over the embedded curve.
    MultiScalarMul {
        points: HeapVector,
        scalars: HeapVector,
        outputs: HeapArray,
    },
    /// Performs addition over the embedded curve.
    EmbeddedCurveAdd {
        input1_x: MemoryAddress,
        input1_y: MemoryAddress,
        input1_infinite: MemoryAddress,
        input2_x: MemoryAddress,
        input2_y: MemoryAddress,
        input2_infinite: MemoryAddress,
        result: HeapArray,
    },
    BigIntAdd {
        lhs: MemoryAddress,
        rhs: MemoryAddress,
        output: MemoryAddress,
    },
    BigIntSub {
        lhs: MemoryAddress,
        rhs: MemoryAddress,
        output: MemoryAddress,
    },
    BigIntMul {
        lhs: MemoryAddress,
        rhs: MemoryAddress,
        output: MemoryAddress,
    },
    BigIntDiv {
        lhs: MemoryAddress,
        rhs: MemoryAddress,
        output: MemoryAddress,
    },
    BigIntFromLeBytes {
        inputs: HeapVector,
        modulus: HeapVector,
        output: MemoryAddress,
    },
    BigIntToLeBytes {
        input: MemoryAddress,
        output: HeapVector,
    },
    Poseidon2Permutation {
        message: HeapVector,
        output: HeapArray,
        len: MemoryAddress,
    },
    Sha256Compression {
        input: HeapArray,
        hash_values: HeapArray,
        output: HeapArray,
    },
    ToRadix {
        input: MemoryAddress,
        radix: MemoryAddress,
        output_pointer: MemoryAddress,
        num_limbs: MemoryAddress,
        output_bits: MemoryAddress,
    },
}
