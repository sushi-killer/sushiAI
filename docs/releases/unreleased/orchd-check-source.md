## orchd may drop a check the planner wrote

- orchd records who wrote each check (planner, repo or owner); a non-owner may drop a planner check, even one that cannot pass on a moved base, while repo and owner checks stay owner-only.
