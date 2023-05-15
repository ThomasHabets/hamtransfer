# HamTransfer

A file transfer program for amateur radio using modern techniques.

Hamtransfer is not a modem, but the protocol on top.

## Status

Currently just has proof of concept code. But speeds are still pretty
good. E.g. better than ZModem over standard 1200/9600bps packet.

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
