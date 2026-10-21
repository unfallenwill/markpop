# Operations Handbook

**Audience:** on-call engineers
**Revision:** 2026-10-01

This handbook covers the runbook for the deployment pipeline, the alert catalogue, and the
escalation path. It is written to be rendered to A4 and printed, so sections are numbered
and tables are kept narrow.

## 1. Deployment pipeline

Every merge to `main` triggers the pipeline below. No stage may be skipped without an
incident record.

| Stage | Timeout | On failure |
|---|---|---|
| `build` | 10 min | retry once, then page |
| `integration` | 20 min | retry twice, then page |
| `canary` | 30 min | auto-rollback, then page |
| `promote` | 10 min | auto-rollback, then page |

### 1.1 Running it locally

```bash
cargo run --release -- pipeline.toml --dry-run
cargo run --release -- pipeline.toml --stage integration
```

The dry run prints the resolved plan without contacting the cluster.

### 1.2 Canary rules

The canary receives 5% of traffic for ten minutes. It is promoted automatically when all
three of the following hold:

- error rate stays below 0.1%;
- p99 latency stays below 250 ms;
- the saturation counter reports no new criticals.

If any condition fails, the deployment rolls back to the previous revision and the on-call
engineer is notified.

> Rollback is always safe. Prefer rolling back and investigating over debugging in place.

## 2. Alert catalogue

| Alert | Severity | Meaning | First action |
|---|---|---|---|
| `pipeline-stalled` | P2 | A stage exceeded its timeout | Check the run log |
| `canary-error-rate` | P1 | Canary error rate above 0.1% | Roll back |
| `saturation-critical` | P1 | Cluster is saturated | Scale out, then roll back |
| `cert-expiry-30d` | P3 | Certificate expires soon | Renew |

### 2.1 Muting

Muting requires a ticket reference in the annotation:

```bash
alertctl mute pipeline-stalled --until 2h --reason "INC-4821"
```

Mutes expire automatically. There is no permanent mute.

## 3. Escalation

1. Primary on-call acknowledges within five minutes.
2. If unacknowledged after fifteen minutes, the secondary is paged.
3. If unacknowledged after thirty minutes, the incident manager is paged.

### 3.1 Contact details

Escalation runs through the on-call rota in the internal directory. Do not keep personal
contact details in this handbook.

## 4. Post-incident checklist

- [ ] Write the timeline within 24 hours
- [ ] Identify the contributing factors, not only the trigger
- [ ] Add a regression test where one is possible
- [ ] Link the follow-up issue from the incident record

## Appendix A. Frequently asked

**Can I skip the canary?** No. Every promotion path includes the canary stage.

**Can I roll back a promotion that already finished?** Yes. Rollback is a supported
operation at any time, and it is expected as a first response.

**What if the rollback itself fails?** Escalate immediately. Do not attempt a manual
rollback on top of a failed automatic rollback.
