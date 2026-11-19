// Package push implements the server-side mobile push registry.
//
// Responsibilities:
//
//   - Device registration backed by the existing Postgres devices table
//     (registrations reference devices.id; see migrations/00061).
//     Registrations are renewable leases: every register/rotate extends the
//     expiry, foreground token rotation is an atomic compare-and-swap, and
//     unregister deletes durably with a periodic expiry cleanup.
//   - Authenticated token leases: registration calls must present the access
//     JWT in the Authorization header (query parameters are refused, H2);
//     the device claim inside it must equal the device being registered.
//     Unauthenticated callers get 401; a device registering a different
//     device's id gets 403.
//   - Sealed host-notify dispatch (E2EE, §8): the desktop seals each
//     notification to the phone's push key and hands the opaque blob to the
//     relay, which passes it to NotifyDispatcher.HandleNotify. The server
//     resolves the phone's live registration and sends one fixed-size push:
//     an APNs alert carrying only the constant placeholder plus the blob,
//     or an FCM data-only HIGH-priority message carrying only the blob. No
//     sender field, no notification text, one push type and priority.
//   - Provider sending behind a Sender interface (APNs/FCM). Credentials come
//     from config/env; with no credentials configured the server logs an
//     aggregate-safe line (platform only, never titles, tokens, blobs or
//     ids) instead of sending. No real credentials live in code or tests.
//
// Mounting and host-notify wiring (left to the integrator — this package
// does not touch main.go or the relay):
//
//	pushHandler := push.NewHandler(store, resolver, push.NewJWTAuthenticator(cfg.JWTSecret))
//	r.Group(func(rr chi.Router) {
//	    rr.Use(RequireAuth)
//	    pushHandler.RegisterRoutes(rr)
//	})
//
// Host-notify wiring: the relay already validates host-notify envelopes
// opaquely and exposes SetE2ENotifyIntake. The integrator connects it with
//
//	notify := &push.NotifyDispatcher{Store: store, Sender: push.SenderFromConfig(push.ConfigFromEnv())}
//	rpcHandler.SetE2ENotifyIntake(notify.HandleNotify)
//
// so sealed desktop blobs reach live phone registrations. Plaintext
// attention events are gone: there is no server-generated push content.
package push
