use std::fs;

use tokio_stream::StreamExt;
use tonic::transport::Channel;

use ax25ms::router_service_client::RouterServiceClient;
// use ax25ms::Frame;
use ax25ms::StreamRequest;

pub mod ax25ms {
    // The string specified here must match the proto package name
    tonic::include_proto!("ax25ms");
}

#[tokio::main]
async fn main() ->  Result<(), Box<dyn std::error::Error>> {
    // Needed input:
    let source_block_size = 6;
    // let total_size = …
    
    // Implementation.
    let mut encoding_symbol_length = 0;
    let mut n = 0u32;
    let mut decoder = raptor_code::SourceBlockDecoder::new(source_block_size);

    let mut client = RouterServiceClient::connect("http://[::1]:12001").await?;
    let mut stream = client.stream_frames(StreamRequest{})
        .await
        .unwrap()
        .into_inner();
    
    while decoder.fully_specified() == false {
        println!("Reading symbol {}", n);
        // 
        // Read from file:
        //let encoding_symbol = fs::read(format!("tmp/{}", n)).expect("read data");
        //
        let frame = stream.next().await.unwrap().unwrap().payload;

        //
        // TODO: parse UI frame.
        //
        
        let encoding_symbol = frame;
        
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
    Ok(())
}
