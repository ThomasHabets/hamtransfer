# HamTransfer

A file transfer program for amateur radio using modern techniques.

Hamtransfer is not a modem, but the protocol on top.

## Status

Currently just has proof of concept code. But speeds are still pretty
good. E.g. better than ZModem over standard 1200/9600bps packet.

## Quick start

This is not the final form of hamtransfer, obviously, but here's a
step by step to testing the current code.

1. Build [ax25ms][ax25ms].
1. Start ax25ms's `./serial -p /dev/rfcomm0 -l '[::]:12001'` (both on
   client and server)
1. Build hamtransfer: `cargo build`
   (binaries end up in `target/debug/` by default)
1. Start uploader, using the uploading ax25ms `serial` port:
   ```
   uploader \
       --router localhost:12001 \
       --parser localhost:12001 \
	   --source M0XXX-1 \
	   --input testdata   # directory with data
   ```
   You'll see a file listing, with checksums.
1. Start downloader, using the downloading ax25ms `serial` port:
   ```
   downloader \
       --router localhost:12001 \
       --parser localhost:12001 \
	   --output test.out
	   checksum-from-the-uploader-file-listing
   ```

## Overall architecture

Because `AF_AX25` sockets are only available on Linux, and are
extremely buggy there too, hamtransfer uses [ax25ms][ax25ms] to
transmit and receive packets.

The interface to ax25ms is gRPC.

If you have a TNC over serial or Bluetooth (E.g. the Kenwood TH-74),
then it's:

```
TNC <bluetooth> ax25ms serial <gRPC> hamtransfer
```

If you don't have a TNC then you'll need to emulate one in software,
for example by using [Direwolf][direwolf].

```
Radio <audio cables> Direwolf <loopback serial> ax25ms serial <gRPC> hamtransfer
```

## Performance

As of 2023-05-28, before the protocol has been fixed to remove a
roundtrip, a 10kB file can go at 5576 bps.

I don't really even understand how it can be that fast, because that
was with 239 byte payloads, which takes ~200ms airtime, and with the
Kenwood D74 set to 200ms TX Delay, it should top out at 4800bps.

With an SDR it looks more like ~10ms of carrier before modulation
starts, so maybe it's 200ms with 1200bps, but only 25ms with 9600?

Reasons why it's not faster, and misc rambling notes:
* One needless roundtrip. Protocol will be fixed to avoid this.
  * This should get the speed up to 6040bps.
* Packet size is not big. Bigger tends to make the D74 crash.
  * Overhead without repeaters is 18 bytes, so with 200 byte payload
    that's 8.25% overhead. 8807bps theoretical max just from that.
  * Time before transmitting:
    * Maybe T107?
    * T102 slot time (randomized) before key-up.
    * TXDELAY is time between key-up and packet.
	  * 300ms by default with direwolf.
	  * [20ms is good. There are ones with 1ms](http://www.symek.com/g/pacmod.html)
	  * Speed formula: (8*size)/(sec+8*size/9600).
	* AXDELAY, if using repeaters. Not used?
    * Optionally plus T105, remote receiver sync.
  * Time after transmitting:
    * TXTail (default 30ms on D74, though I see 11ms with SDR)
    * T108 is time after packet before re-keying.
* Looks like there are gaps between packets when sent via D74.

2023-05-29 experiment with 1200 byte packets for 10kB and removed
extra roundtrip give best run 8024bps. 83% of theoretical max. Most of
that is probably due to the fact that there's always that one
roundtrip.

## Protocol

The protocol is being designed, so no point yet to describe it here.

[direwolf]: https://github.com/wb2osz/direwolf
[ax25ms]: https://github.com/ThomasHabets/ax25ms
