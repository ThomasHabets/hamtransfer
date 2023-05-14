use std::fs;
use ax25ms::router_service_client::RouterServiceClient;
use ax25ms::SendRequest;
use ax25ms::Frame;

pub mod ax25ms {
    // The string specified here must match the proto package name
    tonic::include_proto!("ax25ms");
}

fn float_to_usize(f: f64) -> Option<usize> {
    let ret = f as usize;
    let back = ret as f64;
    if back != f {
        ()
    }
    Some(ret)
}

fn usize_to_float(f: usize) -> Option<f64> {
    let ret = f as f64;
    let back = ret as usize;
    if back != f {
        ()
    }
    Some(ret)
}

#[tokio::main]
async fn main() ->  Result<(), Box<dyn std::error::Error>> {
    let len = 1099;
    let mut source_data: Vec<u8> = vec![1,2,3,4,5,6,7,8,9,10,11,12];
    source_data.resize(len, 0);
    source_data[999] = 98;
    source_data[len-1] = 33;
    println!("{:?}", source_data);

    let packet_size = 200;
    
    let nb_repair = 3;

    let f = (usize_to_float(source_data.len()).unwrap() / usize_to_float(packet_size).unwrap()).ceil();
    let max_source_symbols = float_to_usize(f).unwrap();

    // State that needs to be sent:
    // * max_source_symbols
    // * total size
    // * max_source_symbols is implicitly sent by seeing the block size.
    
    let mut encoder = raptor_code::SourceBlockEncoder::new(&source_data,
                                                           max_source_symbols);
    let n = encoder.nb_source_symbols() + nb_repair;
    for esi in 0..n as u32 {
        let encoding_symbol = encoder.fountain(esi);
    }

    // Transmit RPC.
    let mut client = RouterServiceClient::connect("http://[::1]:12001").await?;
    for esi in 0..n as u32 {
        let encoding_symbol = encoder.fountain(esi);
        let len = encoding_symbol.len();
        //println!("{:?}", encoding_symbol.len());
        //fs::write(format!("tmp/{}", esi), encoding_symbol).expect("write block");
        //
        // TODO: construct AX.25 frame.
        // 
        let request = tonic::Request::new(SendRequest {
            frame: Some(Frame {
                payload: encoding_symbol,
            }),
        });
        client.send(request).await?;
        println!("Sent esi {} of size {}", esi, len);
    }
    Ok(())
}
