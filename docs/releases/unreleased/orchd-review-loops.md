## orchd: review loops

- A retry that changed only test files or `artifacts/` and saved no new image reuses the images of the latest attempt that took its own, instead of failing the evidence gate.
- An implementer can dispute a review finding in its report (`disputes`: the finding, a rebuttal, evidence). When the reviewer repeats a disputed finding, a read-only judge on another harness rules on it once: if it is invalid the finding is dropped as an owner-overturnable assumption and the same attempt is reviewed again; otherwise the advisor, tier-up and wait steps run as before.
