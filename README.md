# katcp_canonicalizer

[![build status](https://img.shields.io/github/workflow/status/GReX-Telescope/katcp/CI/main?style=flat-square&logo=github)](https://github.com/GReX-Telescope/katcp_canonicalizer/actions)

This program creates a TCP proxy that canonicalizes KATCP messages from CASPER's
TCPBORPHServer into spec-compliant katcp.

## The issue

For some reason, TCPBORPHServer uses non-compliant katcp in their `read` and
`write` commands. `read` returns raw bytes instead of plaintext and `write`
accepts raw bytes. Any spec-compliant implementation of katcp (like [this one
in rust](https://github.com/kiranshila/katcp)) will therefore not work with this
server. This is unfortunate as the _only_ katcp client that has these messages
is a monster of a Python 2 program,
[casperfpga](https://github.com/casper-astro/casperfpga), for which many things
are broken.

## The solution

This program acts as middleware and rewrites the `read` and `write` messages to
be base64 encoded. Then, spec-compliant clients can work fine.

## Example

Startup the program with

```sh
./katcp_canonicalizer <proxy_port> <pi_ip>
```

Optionally set the katcp server ip with `--pi_port`, but will default to the
tcpborphserver 7147.

Before canonicalization:

```
?write sys_scratchpad 0 test
!write ok
?read sys_scratchpad 0 4
!read ok test
```

(this might not look like it makes much sense, but raw bytes wouldn't be
representable here (the whole point of this))

After:

```
?write sys_scratchpad 0 dGVzdA==
!write ok
?read sys_scratchpad 0 4
!read ok dGVzdA==
```

## Caveats

There is no error handling (we should probably add some), so any malformed
requests will crash this program
