#![allow(unused)]

use crate::evm::InstructionSeq;

mod program;
mod parser;
mod scope;
mod analysis;
mod codegen;
mod evm;

fn main() {
    let input = "
        let x = const 0;
        let p = const 1;
        mstore p x;
        let y = mload p;
    ";

    let ast = parser::parse(input).unwrap();
    let rblock = scope::resolve(ast).unwrap();
    let code = codegen::generate(&rblock).unwrap();
    let code = InstructionSeq(code.map(|i| i.into()).collect());

    println!("{code}");
}
