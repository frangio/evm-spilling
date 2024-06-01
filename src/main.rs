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
        let p00 = const 10;
        let p01 = const 11;
        let p02 = const 12;
        let p03 = const 13;
        let p04 = const 14;
        let p05 = const 15;
        let p06 = const 16;
        let p07 = const 17;
        let p08 = const 18;
        let p09 = const 19;
        let p10 = const 20;
        let p11 = const 21;
        let p12 = const 22;
        let p13 = const 23;
        let p14 = const 24;
        let p15 = const 25;
        let p16 = const 26;
        let y = mload p00;
        pop p15;
        pop p14;
        pop p13;
        pop p12;
        pop p11;
        pop p10;
        pop p09;
        pop p08;
        pop p07;
        pop p06;
        pop p05;
        pop p04;
        pop p03;
        pop p02;
        pop p01;
        pop p16;
    ";

    let ast = parser::parse(input).unwrap();
    let rblock = scope::resolve(ast).unwrap();
    let code = codegen::generate(&rblock).unwrap();
    let code = InstructionSeq(code.map(|i| i.into()).collect());

    println!("{code}");
}
