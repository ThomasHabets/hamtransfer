use ax25::ax25_parser_client::Ax25ParserClient;
use ax25::packet::FrameType::Ui;
use ax25ms::router_service_client::RouterServiceClient;
use ax25ms::StreamRequest;
use std::fs;
use structopt::StructOpt;
use tokio_stream::StreamExt;

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

#[derive(StructOpt, Debug)]
#[structopt()]
struct Opt {
    #[structopt(short = "r", long = "router")]
    router: String,
    #[structopt(short = "p", long = "parser")]
    parser: String,

    #[structopt(short = "s", long = "source")]
    source: String,

    #[structopt(short = "o", long = "output")]
    output: String,

    #[structopt(short = "d", long = "dst", default_value = "CQ")]
    dst: String,

    // TODO: Use when implementing the requesting protocol.
    // #[structopt(short = "S", long = "source")]
    // source: String,
    filename: String,
}

async fn make_packet(
    parser: &mut Ax25ParserClient<tonic::transport::Channel>,
    dst: &str,
    src: &str,
    payload: String,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let req = tonic::Request::new(ax25::SerializeRequest {
        packet: Some(ax25::Packet {
            dst: dst.to_string(), // TODO: set callsign.
            src: src.to_string(),
            fcs: 0,
            aprs: None,
            repeater: vec![],
            command_response: false,
            command_response_la: true,
            rr_dst1: false,
            rr_extseq: false,
            frame_type: Some(ax25::packet::FrameType::Ui(ax25::packet::Ui {
                pid: 0xF0_i32, // TODO: some protocol ID?
                push: 0,
                payload: payload.into_bytes(),
            })),
        }),
    });
    Ok(parser.serialize(req).await?.into_inner().payload)
}

async fn stream_rpc(
    opt: &Opt,
    decoder: &mut raptor_code::SourceBlockDecoder,
    mut client: RouterServiceClient<tonic::transport::Channel>,
    mut parser: Ax25ParserClient<tonic::transport::Channel>,
    filename: &str,
) -> Result<usize, Box<dyn std::error::Error>> {
    let cmd = make_packet(
        &mut parser,
        &opt.dst,
        &opt.source,
        format!("G 1234 0 0 {}", filename).to_string(),
    )
    .await?;
    client
        .send(tonic::Request::new(ax25ms::SendRequest {
            frame: Some(ax25ms::Frame { payload: cmd }),
        }))
        .await?;

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
            .expect("surely the RPC reply has a packet");

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

#[derive(Debug)]
enum DownloaderError {
    RPCError(tonic::transport::Error),
    StreamError(Box<dyn std::error::Error>),
    ChecksumMismatch(String, String),
}
impl From<Box<dyn std::error::Error>> for DownloaderError {
    fn from(error: Box<dyn std::error::Error>) -> Self {
        DownloaderError::StreamError(error)
    }
}
impl From<tonic::transport::Error> for DownloaderError {
    fn from(error: tonic::transport::Error) -> Self {
        DownloaderError::RPCError(error)
    }
}

impl std::fmt::Display for DownloaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::RPCError(e) => write!(f, "RPC Error: {e}"),
            Self::StreamError(e) => write!(f, "Stream Error: {e}"),
            Self::ChecksumMismatch(chk1, chk2) => write!(f, "Checksum Mismatch: {chk1} != {chk2}"),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), DownloaderError> {
    let opt = Opt::from_args();

    // Needed input:
    // TODO: get this from metadata request.
    let source_block_size = 19;
    let total_size = 3684;

    let mut decoder = raptor_code::SourceBlockDecoder::new(source_block_size);
    println!("Connecting…");
    let client = RouterServiceClient::connect(opt.router.clone()).await?;
    let parser = Ax25ParserClient::connect(opt.parser.clone()).await?;

    println!("Running…");
    let encoding_symbol_length =
        stream_rpc(&opt, &mut decoder, client, parser, &opt.filename).await?;

    println!("Downloaded!");
    let mut source_block = decoder
        .decode(encoding_symbol_length * source_block_size)
        .expect("decode");
    source_block.resize(total_size, 0);

    let digest = sha256::digest(&source_block[..]);
    if digest != opt.filename {
        return Err(DownloaderError::ChecksumMismatch(digest, opt.filename));
    }

    println!("Downloaded size {:?}", source_block.len());
    fs::write(opt.output, source_block).expect("write block");
    Ok(())
}
