fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("proto/api.proto")?;
    tonic_build::compile_protos("proto/ax25.proto")?;
    Ok(())
}
