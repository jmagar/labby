//! Doctor dispatch has no external service client of its own.
//!
//! `system.checks` reads local state (env vars, filesystem).
//! `service.probe` / `audit.full` use `ServiceClients` from the caller.
//! There is no `DOCTOR_URL` env var and no `DoctorClient` constructed here.
