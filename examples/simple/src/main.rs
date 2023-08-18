use anyhow::Result;
use ctrlc;
use openh264::{decoder::Decoder, nal_units};
use rtsp_client::{Methods, Session};
use std::io::Cursor;
use tokio::net::UdpSocket;

#[tokio::main]
async fn main() -> Result<()> {
    let mut rtsp = Session::new("192.168.86.138:554".to_string())?;

    let response = rtsp.send(Methods::Options).await?;
    println!("OPTIONS: \n{response}");

    let response = rtsp.send(Methods::Describe).await?;
    println!("DESCRIBE: \n{response}");

    let response = rtsp.send(Methods::Setup).await?;
    println!("SETUP: \n{response}");

    let response = rtsp.send(Methods::Play).await?;
    println!("PLAY: \n{response}");

    if (&response).contains("200 OK") {
        // Bind to my client UDP port which is provided in DESCRIBE method
        // in the 'Transport' header
        let udp_stream = UdpSocket::bind("0.0.0.0:4588").await?;

        // Connect to the RTP camera server using IP and port
        // provided in SETUP response
        // In the RTP specs, the RTCP server should be
        // port 6601 and will always need to be
        // a different port
        udp_stream.connect("192.168.86.138:6600").await?;

        // Set buffer to large enough to handle RTP packets
        // in my Wireshark analysis for this camera they
        // tended be a bit more than 1024
        let mut buf_rtp = [0u8; 2048];

        // Keep a separate buffer for the NAL units
        // which should be the payload of each
        // RTP packet. Some NAL units may not
        // contain enough info on their own and
        // may need more units, hence the buffer
        let mut payload: Vec<u8> = Vec::new();

        // Capture X num fragments and then exit
        let mut sequence_started = false;

        // Packet sequence for RTP using H264 and
        // packetization-mode=1 (non-interleaved mode)
        // Seems to go like this:
        //
        // Packet 1 - SPS (NAL Type 7) ---------------------|
        // Packet 2 - PPS (NAL Type 8)                      |
        // Packet 3 - SEI (NAL Type 6)                      |
        // Packet 4 - FU-A (NAL Type 28) Start              |-- First Packet Sequence
        // Packet 5 - FU-A (NAL Type 28)                    |
        // Packet 6 - FU-A (NAL Type 28) End                |
        // Packet 7 - Coded Slice Non-IDR (NAL Type 1)      |
        // Packet 8+ - More Coded Slices (NAL Type 1)-------|
        //
        // Packet 1 - SPS (NAL Type 7)----------------------|
        // Packet 2 - PPS (NAL Type 8)                      |
        // Packet 3 - SEI (NAL Type 6)                      |
        // Packet 4 - FU-A (NAL Type 28) Start              |-- Second Packet Sequence, etc.
        // Packet 5 - FU-A (NAL Type 28)                    |
        // Packet 6 - FU-A (NAL Type 28) End                |
        // Packet 7 - Coded Slice Non-IDR (NAL Type 1)      |
        // Packet 8+ - More Coded Slices (NAL Type 1)-------|
        loop {
            let len = udp_stream.recv(&mut buf_rtp).await?;
            let header_nal = &buf_rtp[12];

            println!("{} bytes received", len);
            println!("-----------\n{:08b}", header_nal);

            // Check if this is an SPS packet
            // First byte should be -> 01100111
            if *header_nal == 103u8 {
                if sequence_started {
                    // This is the end of the previous sequence
                    // Attempt to decode with H264
                    let mut decoder = Decoder::new()?;
                    match decoder.decode(payload.as_slice()) {
                        Ok(maybe_yuv) => match maybe_yuv {
                            Some(yuv) => println!("Decoded YUV!"),
                            None => println!("Unable to decode to YUV"),
                        },
                        Err(e) => eprintln!("Decoding error: {e}"),
                    }

                    break;
                } else {
                    sequence_started = true;
                }
            }

            // If packetization-mode=1 found in SDP of Describe
            // this is non-interleaved mode
            // AND we find a packet of NAL type 28
            // then server is sending FU-A type fragments
            // AND each FU fragment has 2 byte headers
            // So, NAL header 01111100 denotes FU-A fragment
            if *header_nal == 124u8 {
                // Get the 2nd byte for more header info
                let header_fu = &buf_rtp[13];
                println!("FU Header -----------\n{:08b}", header_fu);

                // Add FU payload to buffer which is
                // RTP packet minus RTP header minus FU header
                // = packet - 12u8 - 2u8
                // = packet - 14
                payload.extend_from_slice(&buf_rtp[14..len]);
                println!("FRAGMENT packet received. Buffer length: {}", payload.len());

                // Look for an IDR fragment
                // which is detemined by NAL type in last 5 bits
                // IDR is NAL type 5 which is 101 for last 5 bits

                // FU header = 10000101 -- fragment start
                // FU header = 00000101 -- fragment middle
                // FU header = 01000101 -- fragment end
                if *header_fu == 133u8 || *header_fu == 69u8 || (*header_fu == 5u8) {
                    // End of fragment, try to decode
                    if *header_fu == 69u8 {}
                }
            } else {
                // First 12 bytes AT LEAST are for the RTP
                // header and this header can be longer
                // depending on CC flag bit
                // header.len() == 12 + (CC * 4)
                payload.extend_from_slice(&buf_rtp[12..len]);
                println!("Non fragment packet. Buffer length: {}", payload.len());
            }
        }
    }

    println!("Stopping RTSP: {}", rtsp.stop());
    Ok(())
}
