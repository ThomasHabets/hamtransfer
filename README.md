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
1. Start downloader, using the downloading ax25ms `serial` port:
   ```
   downloader \
       --router localhost:12001 \
       --parser localhost:12001 \
	   --output test.out
   ```
1. Start uploader, using the uploading ax25ms `serial` port:
   ```
   uploader \
       --router localhost:12001 \
       --parser localhost:12001 \
	   --source M0XXX-1 \
	   --input README.md
   ```

If download doesn't complete, just run uploader again without
restarting the downloader. The downloader will exit when it has the
whole file.

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

## Protocol

The protocol is being designed, so no point yet to describe it here.

[direwolf]: https://github.com/wb2osz/direwolf
[ax25ms]: https://github.com/ThomasHabets/ax25ms
