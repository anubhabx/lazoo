# Infrastructure resource governor

The runtime creates one owner-protected named infrastructure Job Object per installation UUID. Windows must confirm a 20% CPU hard cap and silent breakaway before it is reported as enforced. Enrollment is explicit; dropping a handle does not kill the job.

The daemon, keeper, and CLI now enroll in that shared job. Provider descendants break away from the infrastructure job and remain in their own containment jobs. The desktop is not enrolled and `system.budget` reports `desktopIncluded: false` and `providerTreesIncluded: false`.

The daemon samples aggregate CPU at one-second intervals, coalesces bounded buckets, and reports warming/ready state for 30-second and five-minute windows. Background archive and summary admission pauses at the 15% soft threshold and fails closed when measurements are unavailable. The durable summary queue provides its own capacity and rate admission; the runtime BackgroundQueue primitive is not its scheduler.

Runtime tests cover hard-cap query/enrollment, separate jobs, rolling-window math and bounded sampling. `scripts/acceptance/core-services.ps1` checks current real binaries with disposable state and bounded terminal output. Whole-product baseline performance, desktop inclusion, five-minute loaded acceptance, and memory limits remain separate from these verified boundaries. See `2026-10-15-core-v1-backend-status.md` for final gate evidence.
