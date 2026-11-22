use std::collections::HashMap;

pub struct Config {
    pub name: String,
    pub retries: u32,
    pub mode: Mode,
    pub limits: Option<(u8, u16)>,
}

pub enum Mode {
    Off,
    Fast(u8),
    Custom { level: i64, label: &'static str },
}

pub struct Wrapper<T> {
    pub inner: T,
    pub count: usize,
}

pub struct Node {
    pub value: i32,
    pub next: Option<Box<Node>>,
}

pub type Scores = HashMap<String, Vec<f64>>;

pub fn configure(cfg: &Config, overrides: Wrapper<bool>) {}

pub fn generic<T: Clone + Into<u64>, K>(items: Vec<T>, key: K, list: &Node)
where
    K: std::hash::Hash + Eq,
{
}

pub fn scores(s: Scores, f: impl Fn(u8) -> bool, xs: &[char; 4]) {}

impl<T: Default> Wrapper<T> {
    pub fn set(&mut self, value: T) {}
}
