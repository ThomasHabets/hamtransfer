use ax25::ax25_parser_client::Ax25ParserClient;

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

/*
* make a UI packet with given payload
*/
pub async fn make_packet(
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
