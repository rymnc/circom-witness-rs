use crate::field::M;
use ark_bn254::Fr;
use ark_ff::{BigInt, PrimeField, Zero};
use rand::Rng;
use ruint::aliases::U256;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ops::{BitAnd, Shl, Shr},
};

use ark_serialize::{CanonicalDeserialize, CanonicalSerialize, Compress, Validate};

fn ark_se<S, A: CanonicalSerialize>(a: &A, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let mut bytes = vec![];
    a.serialize_with_mode(&mut bytes, Compress::Yes)
        .map_err(serde::ser::Error::custom)?;
    s.serialize_bytes(&bytes)
}

fn ark_de<'de, D, A: CanonicalDeserialize>(data: D) -> Result<A, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let s: Vec<u8> = serde::de::Deserialize::deserialize(data)?;
    let a = A::deserialize_with_mode(s.as_slice(), Compress::Yes, Validate::Yes);
    a.map_err(serde::de::Error::custom)
}

#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Operation {
    Mul,
    MMul,
    Add,
    Sub,
    Eq,
    Neq,
    Lt,
    Gt,
    Leq,
    Geq,
    Lor,
    Shl,
    Shr,
    Band,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Node {
    Input(usize),
    Constant(U256),
    #[serde(serialize_with = "ark_se", deserialize_with = "ark_de")]
    MontConstant(Fr),
    Op(Operation, usize, usize),
}

impl Operation {
    pub fn eval(&self, a: U256, b: U256) -> U256 {
        use Operation::*;
        match self {
            Add => a.add_mod(b, M),
            Sub => a.add_mod(M - b, M),
            Mul => a.mul_mod(b, M),
            Eq => U256::from(a == b),
            Neq => U256::from(a != b),
            Lt => U256::from(a < b),
            Gt => U256::from(a > b),
            Leq => U256::from(a <= b),
            Geq => U256::from(a >= b),
            Lor => U256::from(a != U256::ZERO || b != U256::ZERO),
            Shl => compute_shl_uint(a, b),
            Shr => compute_shr_uint(a, b),
            Band => a.bitand(b),
            _ => unimplemented!("operator {:?} not implemented", self),
        }
    }

    pub fn eval_fr(&self, a: Fr, b: Fr) -> Fr {
        use Operation::*;
        match self {
            Add => a + b,
            Sub => a - b,
            Mul => a * b,
            Eq => (a == b).into(),
            Neq => (a != b).into(),
            Lt => (a < b).into(),
            Gt => (a > b).into(),
            Leq => (a <= b).into(),
            Geq => (a >= b).into(),
            Lor => (a != Fr::zero() || b != Fr::zero()).into(),
            Shl => compute_shl_Fr(a, b),
            Shr => compute_shr_Fr(a, b),
            Band => compute_bitand_Fr(a, b),
            _ => unimplemented!("operator {:?} not implemented for Montgomery", self),
        }
    }
}

fn compute_shl_uint(a: U256, b: U256) -> U256 {
    debug_assert!(b.lt(&U256::from(256)));
    let ls_limb = b.as_limbs()[0];
    a.shl(ls_limb as usize)
}

fn compute_shr_uint(a: U256, b: U256) -> U256 {
    debug_assert!(b.lt(&U256::from(256)));
    let ls_limb = b.as_limbs()[0];
    a.shr(ls_limb as usize)
}

fn u8_to_u64_array(input: &[u8]) -> [u64; 4] {
    // Check that the length of the input array is exactly 32 bytes
    assert!(input.len() == 32, "Input length must be exactly 32 bytes");

    // Create an array to hold the converted values
    let mut output = [0u64; 4];

    // Iterate over chunks of 8 bytes and convert them to u64
    for (i, chunk) in input.chunks_exact(8).enumerate() {
        let mut array = [0u8; 8];
        array.copy_from_slice(chunk);
        output[i] = u64::from_le_bytes(array);
    }

    output
}

pub fn compute_shl_Fr(a: Fr, b: Fr) -> Fr {
    // convert Fr to U256
    let a: U256 = a.into();
    let b: U256 = b.into();
    let result = compute_shl_uint(a, b);
    let bytes: [u8; 64] = result.to_le_bytes();
    Fr::from(BigInt::new(u8_to_u64_array(&bytes)))
}

pub fn compute_shr_Fr(a: Fr, b: Fr) -> Fr {
    let a: U256 = a.into();
    let b: U256 = b.into();
    let result = compute_shr_uint(a, b);
    let bytes: [u8; 64] = result.to_le_bytes();
    Fr::from(BigInt::new(u8_to_u64_array(&bytes)))
}

pub fn compute_bitand_Fr(a: Fr, b: Fr) -> Fr {
    let a: U256 = a.into();
    let b: U256 = b.into();
    let result = a.bitand(b);
    let bytes: [u8; 64] = result.to_le_bytes();
    Fr::from(BigInt::new(u8_to_u64_array(&bytes)))
}

/// All references must be backwards.
fn assert_valid(nodes: &[Node]) {
    for (i, &node) in nodes.iter().enumerate() {
        if let Node::Op(_, a, b) = node {
            assert!(a < i);
            assert!(b < i);
        }
    }
}

pub fn optimize(nodes: &mut Vec<Node>, outputs: &mut [usize]) {
    tree_shake(nodes, outputs);
    propagate(nodes);
    value_numbering(nodes, outputs);
    constants(nodes);
    tree_shake(nodes, outputs);
    montgomery_form(nodes);
}

pub fn evaluate(nodes: &[Node], inputs: &[U256], outputs: &[usize]) -> Vec<U256> {
    // assert_valid(nodes);

    // Evaluate the graph.
    let mut values = Vec::with_capacity(nodes.len());
    for (_, &node) in nodes.iter().enumerate() {
        let value = match node {
            Node::Constant(c) => Fr::new(c.into()),
            Node::MontConstant(c) => c,
            Node::Input(i) => Fr::new(inputs[i].into()),
            Node::Op(op, a, b) => op.eval_fr(values[a], values[b]),
        };
        values.push(value);
    }

    // Convert from Montgomery form and return the outputs.
    let mut out = vec![U256::ZERO; outputs.len()];
    for i in 0..outputs.len() {
        out[i] = U256::try_from(values[outputs[i]].into_bigint()).unwrap();
    }

    out
}

/// Constant propagation
pub fn propagate(nodes: &mut [Node]) {
    assert_valid(nodes);
    let mut constants = 0_usize;
    for i in 0..nodes.len() {
        if let Node::Op(op, a, b) = nodes[i] {
            if let (Node::Constant(va), Node::Constant(vb)) = (nodes[a], nodes[b]) {
                nodes[i] = Node::Constant(op.eval(va, vb));
                constants += 1;
            } else if a == b {
                // Not constant but equal
                use Operation::*;
                if let Some(c) = match op {
                    Eq | Leq | Geq => Some(true),
                    Neq | Lt | Gt => Some(false),
                    _ => None,
                } {
                    nodes[i] = Node::Constant(U256::from(c));
                    constants += 1;
                }
            }
        }
    }

    eprintln!("Propagated {constants} constants");
}

/// Remove unused nodes
pub fn tree_shake(nodes: &mut Vec<Node>, outputs: &mut [usize]) {
    assert_valid(nodes);

    // Mark all nodes that are used.
    let mut used = vec![false; nodes.len()];
    for &i in outputs.iter() {
        used[i] = true;
    }

    // Work backwards from end as all references are backwards.
    for i in (0..nodes.len()).rev() {
        if used[i] {
            if let Node::Op(_, a, b) = nodes[i] {
                used[a] = true;
                used[b] = true;
            }
        }
    }

    // Remove unused nodes
    let n = nodes.len();
    let mut retain = used.iter();
    nodes.retain(|_| *retain.next().unwrap());
    let removed = n - nodes.len();

    // Renumber references.
    let mut renumber = vec![None; n];
    let mut index = 0;
    for (i, &used) in used.iter().enumerate() {
        if used {
            renumber[i] = Some(index);
            index += 1;
        }
    }
    assert_eq!(index, nodes.len());
    for (&used, renumber) in used.iter().zip(renumber.iter()) {
        assert_eq!(used, renumber.is_some());
    }

    // Renumber references.
    for node in nodes.iter_mut() {
        if let Node::Op(_, a, b) = node {
            *a = renumber[*a].unwrap();
            *b = renumber[*b].unwrap();
        }
    }
    for output in outputs.iter_mut() {
        *output = renumber[*output].unwrap();
    }

    eprintln!("Removed {removed} unused nodes");
}

/// Randomly evaluate the graph
fn random_eval(nodes: &mut Vec<Node>) -> Vec<U256> {
    let mut rng = rand::thread_rng();
    let mut values = Vec::with_capacity(nodes.len());
    let mut inputs = HashMap::new();
    let mut prfs = HashMap::new();
    for node in nodes.iter() {
        use Operation::*;
        let value = match node {
            // Constants evaluate to themselves
            Node::Constant(c) => *c,

            Node::MontConstant(c) => unimplemented!("should not be used"),

            // Algebraic Ops are evaluated directly
            // Since the field is large, by Swartz-Zippel if
            // two values are the same then they are likely algebraically equal.
            Node::Op(op @ (Add | Sub | Mul), a, b) => op.eval(values[*a], values[*b]),

            // Input and non-algebraic ops are random functions
            // TODO: https://github.com/recmo/uint/issues/95 and use .gen_range(..M)
            Node::Input(i) => *inputs.entry(*i).or_insert_with(|| rng.gen::<U256>() % M),
            Node::Op(op, a, b) => *prfs
                .entry((*op, values[*a], values[*b]))
                .or_insert_with(|| rng.gen::<U256>() % M),
        };
        values.push(value);
    }
    values
}

/// Value numbering
pub fn value_numbering(nodes: &mut Vec<Node>, outputs: &mut [usize]) {
    assert_valid(nodes);

    // Evaluate the graph in random field elements.
    let values = random_eval(nodes);

    // Find all nodes with the same value.
    let mut value_map = HashMap::new();
    for (i, &value) in values.iter().enumerate() {
        value_map.entry(value).or_insert_with(Vec::new).push(i);
    }

    // For nodes that are the same, pick the first index.
    let mut renumber = Vec::with_capacity(nodes.len());
    for value in values {
        renumber.push(value_map[&value][0]);
    }

    // Renumber references.
    for node in nodes.iter_mut() {
        if let Node::Op(_, a, b) = node {
            *a = renumber[*a];
            *b = renumber[*b];
        }
    }
    for output in outputs.iter_mut() {
        *output = renumber[*output];
    }

    eprintln!("Global value numbering applied");
}

/// Probabilistic constant determination
pub fn constants(nodes: &mut Vec<Node>) {
    assert_valid(nodes);

    // Evaluate the graph in random field elements.
    let values_a = random_eval(nodes);
    let values_b = random_eval(nodes);

    // Find all nodes with the same value.
    let mut constants = 0;
    for i in 0..nodes.len() {
        if let Node::Constant(_) = nodes[i] {
            continue;
        }
        if values_a[i] == values_b[i] {
            nodes[i] = Node::Constant(values_a[i]);
            constants += 1;
        }
    }
    eprintln!("Found {} constants", constants);
}

/// Convert to Montgomery form
pub fn montgomery_form(nodes: &mut [Node]) {
    for node in nodes.iter_mut() {
        use Node::*;
        match node {
            Constant(c) => *node = MontConstant(Fr::new((*c).into())),
            MontConstant(..) => (),
            Input(..) => (),
            Op(..) => (),
        }
    }
    eprintln!("Converted to Montgomery form");
}
