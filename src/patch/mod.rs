// src/patch/mod.rs — Patch: the identity half of Phase 6
//
// "Patch" is the vocabulary, settled 2026-08-22: you *patch into* someone's
// deck, the shared timeline is the Reel, a joint sitting is a Shared Spool,
// passing someone a record is a Dub. This module is the part of it that faces
// the network — the actor, its keypair, and the signatures that make anything
// it says believable.
//
// **Local-first, and this is the local half.** Actor endpoints, WebFinger and
// signature verification serve from day one; nothing here delivers anything
// outbound, and there is no UI. That ordering is deliberate: the identity is
// the thing other servers cache, so it has to be right before anybody holds a
// copy of it, and it is the only part of Phase 6 that cannot be changed later
// without breaking someone else's database.
//
// Three files, split by what fails silently in each:
//
// * `keys` — an identity is a keypair, and the failure is that a wrong key
//   verifies nothing while looking exactly like a forgery.
// * `signatures` — the trust boundary. Every rule about whether to believe a
//   request is pure, for the reason `own_history_tracks` and `public_payload`
//   are pure: a rule that decides what is let in must be testable without a
//   server.
// * `actor` — the documents. Their *shape* is the interoperability surface,
//   which is the thing that fails with a 200 and no error anywhere.

pub mod actor;
pub mod keys;
pub mod signatures;
