use std::fs;

use tokio_stream::StreamExt;

use ax25::ax25_parser_client::Ax25ParserClient;
use ax25::packet::FrameType::Ui;
use ax25ms::router_service_client::RouterServiceClient;
use ax25ms::StreamRequest;

pub mod ax25ms {
    // The string specified here must match the proto package name
    tonic::include_proto!("ax25ms");
}

pub mod ax25 {
    tonic::include_proto!("ax25");
}
pub mod aprs {
    tonic::include_proto!("aprs");
}

async fn _stream_files(
    decoder: &mut raptor_code::SourceBlockDecoder,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut encoding_symbol_length = 0;
    let mut esi = 0;
    while !decoder.fully_specified() {
        let encoding_symbol = fs::read(format!("tmp/{}", esi)).expect("read data");
        println!("Got id {}", esi);

        encoding_symbol_length = encoding_symbol.len();
        decoder.push_encoding_symbol(&encoding_symbol, esi);
        esi += 1;
    }
    Ok(encoding_symbol_length)
}

async fn stream_rpc(
    decoder: &mut raptor_code::SourceBlockDecoder,
) -> Result<usize, Box<dyn std::error::Error>> {
    println!("Connecting…");
    let mut client = RouterServiceClient::connect("http://[::1]:13001").await?;
    let mut parser = Ax25ParserClient::connect("http://[::1]:13001").await?;

    println!("Starting stream…");
    let mut stream = client
        .stream_frames(StreamRequest {})
        .await
        .unwrap()
        .into_inner();

    println!("Awaiting data…");
    let mut encoding_symbol_length = 0;
    while !decoder.fully_specified() {
        //
        // Read from file:
        //let encoding_symbol = fs::read(format!("tmp/{}", n)).expect("read data");
        //
        let frame = stream.next().await.unwrap().unwrap().payload;

        //
        // Parse UI frame.
        //
        let parsed = parser
            .parse(tonic::Request::new(ax25::ParseRequest { payload: frame }))
            .await?
            .into_inner()
            .packet
            .unwrap();

        let ui = match parsed.frame_type {
            Some(Ui(ui)) => ui,
            _ => continue,
        };
        println!("Got id {}", ui.pid);
        let encoding_symbol = ui.payload;

        encoding_symbol_length = encoding_symbol.len();
        let esi = ui.pid as u32;
        decoder.push_encoding_symbol(&encoding_symbol, esi);
    }
    Ok(encoding_symbol_length)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Needed input:
    let source_block_size = 19;
    let total_size = 3684;

    // Implementation.
    let mut decoder = raptor_code::SourceBlockDecoder::new(source_block_size);

    let encoding_symbol_length = stream_rpc(&mut decoder).await?;
    //let encoding_symbol_length = _stream_files(&mut decoder).await?;

    println!("Fully specified!");
    let source_block_size = encoding_symbol_length * source_block_size;
    let mut source_block = decoder.decode(source_block_size).expect("decode");
    source_block.resize(total_size, 0);
    println!("{:?}", source_block);
    println!("{:?}", source_block.len());
    fs::write("received.dat", source_block).expect("write block");
    Ok(())
}
