use alloy_primitives::U256;

#[derive(Debug, Clone)]
pub enum Expression<V> {
    Const(U256),
    Op(String, Vec<V>),
}

#[derive(Debug)]
pub struct Statement<V>(pub Vec<V>, pub Expression<V>);

#[derive(Debug)]
pub struct Block<V>(pub Vec<Statement<V>>);
