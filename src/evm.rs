use std::{fmt::Display, str::FromStr};

use eyre::{eyre, Error, Ok, Result, Report};
use alloy_primitives::U256;

#[derive(Debug, Clone)]
pub enum Instruction {
    Stack(StackInstruction),
    Control(ControlInstruction),
    Data(DataInstruction),
}

#[derive(Debug, Clone)]
pub enum StackInstruction {
    Dup(usize),
    Swap(usize),
    Push(Box<U256>),
}

#[derive(Debug, Clone)]
pub enum ControlInstruction {
    Jump(usize),
    Jumpi(usize),
    Jumpdest,
}

#[derive(Debug, Clone, Copy)]
pub enum DataInstruction {
    Pop, // considered data no-op
    Mstore,
    Mload,
    Add,
}

impl DataInstruction {
    pub fn arity(&self) -> (usize, usize) {
        use DataInstruction::*;
        match self {
            Pop => (1, 0),
            Mstore => (2, 0),
            Mload => (1, 1),
            Add => (2, 1),
        }
    }
}

impl FromStr for DataInstruction {
    type Err = Report;

    fn from_str(op: &str) -> Result<Self> {
        use DataInstruction::*;
        match op {
            "pop" => Ok(Pop),
            "mstore" => Ok(Mstore),
            "mload" => Ok(Mload),
            "add" => Ok(Add),
            _ => Err(eyre!("Unknown operator: {op}")),
        }
    }
}

impl Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use DataInstruction::*;
        use StackInstruction::*;
        match self {
            Instruction::Stack(Dup(i)) => write!(f, "dup{}", i + 1),
            Instruction::Stack(Swap(i)) => write!(f, "swap{i}"),
            Instruction::Stack(Push(c)) if c.is_zero() => write!(f, "push0"),
            Instruction::Stack(Push(c)) => write!(f, "push{} {c}", c.byte_len()),
            Instruction::Data(Pop) => write!(f, "pop"),
            Instruction::Data(Mstore) => write!(f, "mstore"),
            Instruction::Data(Mload) => write!(f, "mload"),
            Instruction::Data(Add) => write!(f, "add"),
            Instruction::Control(_) => todo!(),
        }
    }
}

pub struct InstructionSeq(pub Vec<Instruction>);

impl Display for InstructionSeq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for instr in &self.0 {
            write!(f, "{instr}\n")?
        }
        std::fmt::Result::Ok(())
    }
}
