pub struct DnaSequence(pub String);

pub struct Read {
    pub id: u32,
    pub sequence: DnaSequence,
    pub quality: Vec<u8>,
}

pub fn gc_content(seq: &DnaSequence) -> f64 {
    0.0
}

pub fn align(reference: &DnaSequence, reads: &[Read]) {}
