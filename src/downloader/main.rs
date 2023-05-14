use std::fs;

fn main() {
    // Needed input:
    let source_block_size = 6;
    // let total_size = …
    
    // Implementation.
    let mut encoding_symbol_length = 0;
    let mut n = 0u32;
    let mut decoder = raptor_code::SourceBlockDecoder::new(source_block_size);

    while decoder.fully_specified() == false {
        println!("Reading symbol {}", n);
        let encoding_symbol = fs::read(format!("tmp/{}", n)).expect("read data");
        encoding_symbol_length = encoding_symbol.len();
        let esi = n;
        decoder.push_encoding_symbol(&encoding_symbol, esi);
        n += 1;
    }
    println!("Fully specified!");
    let source_block_size = encoding_symbol_length * source_block_size;
    let source_block = decoder.decode(source_block_size as usize).expect("decode");
    println!("{:?}", source_block);
    println!("{:?}", source_block.len());
}
