# Ordinary SDK HTTP/1 ownership and retirement evidence

The native route is the public SDK client through its ordinary HttpTransport and
reqwest/Rustls, with fixed official logical API/media URLs. A normal proxy sends
CONNECT only to an owned loopback helper. Only the generated test root is trusted
by that test client. The helper is known repository source built with official Go
1.27.1 and the standard library; it never forwards CONNECT outside loopback.

## Exact runnable scope

Seven Rust tests execute nine native schedules and one scope assertion:

- Five portable schedules from the sealed nine-row `client-idle-native-h1.json`:
  active headers, active body/repeated close, new-acquisition reset and exact reuse,
  mixed idle/active repeated close, and resource EOF after close
- Four `client-ownership.json` native lifecycle schedules: owned independent pools
  and caller-owned shared pools, each with and without positive SDK pacing
- One non-native assertion preserves all nine sealed rows and explicitly names the
  four current Context/explicit Body.Close gaps

The five H1 schedules compare exact public SDK successes, IDs, statuses and bytes,
exact request connection IDs/method/official host/target/HTTP/1.1, close-idle call
counts, and independently observed peer shutdown at the sealed barriers. The four
ownership schedules compare all six request connection IDs and authentication
headers and exact pre-cleanup shutdown counts. No fixed sleeps, added requests,
retry-based relaxation, fixture changes or ignored tests are used.

## What proves shutdown

The current helper observes EOF or ECONNRESET on the raw TCP reader beneath TLS.
When normal server closure follows TLS close_notify, it drains only remaining
owned raw bytes, with a ten-second deadline, before local close. TLS close_notify,
SDK completion and server handler completion are not counted as physical TCP
shutdown. Owned helper stop is excluded from the observation. Rust owns and
bounds helper cleanup, waits the child, joins its stdout reader and removes its
per-test directory.

The prior passing helper observed TLS-layer EOF. Its source snapshots, source
hashes and nine raw witnesses are retained in `superseded-tls-layer/`. That run
established the portable request/public SDK outcomes, but is not independent raw
TCP shutdown evidence. The current passing run and its raw-TCP witnesses are in
`replays/native-raw-first.log` and `native-raw-first/`. Twenty source-sealed
repeats with four Rust test threads also passed, producing 180 schedule witnesses
in `native-raw-repeat-20/`; the combined log additionally records ten successful
13-test bare-reqwest runs owned by the separate core worker. Those core tests are
not counted as SDK schedules.

## Explicit limits

Go-private PutIdleConn callbacks, acceptance and error strings are not compared.
Hyper-util returns H1 senders through a separately spawned on_idle future, so a
public SDK EOF completion is not asserted to establish pool insertion. Actual
reuse is proved only by the next real SDK request's exact frozen connection ID.

The current ResourceBody has no corresponding fallible Go Body.Close API; even
EOF-row close_success is excluded rather than equating a Rust drop with Close.
Ordinary SDK requests/resource bodies do not carry caller Context, so the three
cancellation rows are not mapped to future abort. The early-Close row is also
unmapped. All four Go rows remain unchanged and visible.

Owned lifecycle schedules use explicit ownership transfer of a normally built
custom native client, not a claimed synthetic execution of the default
constructor. No Windows/Darwin, HTTP/2, multiplexing, unfinished HEAD, uploads,
previously denied probes, real accounts/media, OS trust or environment/registry/
security changes are claimed. The unchanged full Rust gate remains the parent's
separate validation obligation. `mapping.json` records current commands, status,
source hashes and precise exclusions.
