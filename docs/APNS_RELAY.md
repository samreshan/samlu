# Hosted APNs relay architecture

Status: design only. The iPhone app and relay are intentionally deferred until
the macOS build is stable and Apple signing is available.

## Release boundary

The first mobile version supports one paired Mac per iPhone. It receives
configurable completion and input-needed notifications from Samlu. Pairing a
new Mac replaces the previous pairing.

The hosted service routes messages through Apple Push Notification service
(APNs), but it must not be able to read agent summaries or other notification
content.

## Components

```text
Claude Code / Codex
        |
        v
Samlu on Mac
  normalize event
  apply notification preferences
  encrypt payload for paired iPhone
        |
        v
Hosted relay
  authenticate device
  rate limit and enqueue ciphertext
        |
        v
APNs
        |
        v
Samlu on iPhone
  decrypt locally
  render notification
```

## Pairing

1. The iPhone creates a Curve25519 encryption key pair in Keychain. The private
   key is non-exportable where platform APIs permit.
2. The Mac creates its own pairing key pair in macOS Keychain.
3. The iPhone displays a short-lived QR pairing payload containing the relay
   pairing ID, iPhone public key, relay origin, and an expiry.
4. The Mac scans the payload, signs the pairing request, and uploads only
   public keys and routing metadata.
5. Both devices derive a shared secret with X25519 and HKDF-SHA-256. A
   confirmation code derived from the transcript is shown on both devices.
6. Accepting a new Mac revokes the old Mac-to-iPhone pairing and rotates relay
   credentials.

The relay stores a random pairing identifier, APNs device token, public keys,
credential hashes, timestamps, and delivery state. It never receives a shared
secret or private key.

## Payload encryption

Before upload, the Mac serializes a versioned payload:

```json
{
  "version": 1,
  "eventId": "uuid",
  "kind": "completed",
  "agent": "codex",
  "project": "samlu-companion",
  "summary": "Optional, controlled by the Mac setting",
  "createdAt": "RFC3339 timestamp"
}
```

The Mac encrypts it with ChaCha20-Poly1305 using a fresh random nonce and the
pairing-derived content key. Associated data includes the protocol version,
pairing ID, event ID, and creation timestamp. The APNs body contains only a
generic alert plus ciphertext, nonce, and routing identifiers.

Agent summaries are included before encryption only when the app preference is
enabled. The hosted relay therefore cannot inspect summary content.

## Delivery behavior

- Use APNs background or notification-service processing to decrypt and render
  the final local notification.
- Fall back to a generic "Agent update available" alert if decryption is not
  possible within the platform deadline.
- Keep ciphertext briefly for retry, then delete it after delivery or expiry.
- Use event IDs for replay protection and deduplication.
- Reject payloads outside a small clock-skew window.
- Rate limit by pairing and source credential.

## Hosted service API

The minimum relay endpoints are:

- `POST /v1/pairings` creates a short-lived pairing session.
- `POST /v1/pairings/{id}/complete` exchanges public pairing material.
- `PUT /v1/devices/{id}/apns-token` rotates the APNs device token.
- `POST /v1/events` accepts authenticated encrypted payloads.
- `DELETE /v1/pairings/{id}` revokes a pairing and deletes queued ciphertext.

Requests from the Mac should use a rotated device credential stored in
Keychain. Production transport requires TLS, certificate validation, request
size limits, replay protection, audit metadata without payload content, and
server-side rate limiting.

## Decisions still required

- Relay hosting provider and operational region
- APNs token-based authentication and secret rotation
- Background push versus notification service extension behavior
- Recovery when either device loses its Keychain material
- Retention period for undelivered ciphertext
- Account-free pairing versus an optional user account

No relay code should ship until these decisions, an abuse model, and Apple
notification entitlement testing are complete.
