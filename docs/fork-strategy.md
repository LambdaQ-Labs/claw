# Claw — Roc Fork Strategy

## Decision: vendored hard fork, with an upstream remote for cherry-picks

Claw diverges heavily from Roc (code-as-database, constraint server, contracts, `--emit=rust`). This is not a patch set on top of Roc — it's a descendant. So:

- **Vendor Roc's source into `compiler/`** (not a submodule). We own it, rename freely, restructure at will.
- **Keep an `upstream` git remote** pointing at `roc-lang/roc`. Periodically review upstream commits; cherry-pick fixes we want (type-inference bugfixes, backend improvements) into `compiler/`.
- **Do NOT** try to stay mergeable with upstream long-term. The divergence is the point. Cherry-pick, don't rebase.

### Why not submodule / subtree
- Submodule: pins us to upstream's structure; our renames + restructuring fight it constantly.
- Subtree: better, but still assumes we track upstream's layout. We won't.
- Vendored fork: max freedom, at the cost of manual cherry-picking. Correct for a language that intends to diverge.

## Bootstrap steps (P0)

```bash
# 1. get Roc source
git clone https://github.com/roc-lang/roc.git /tmp/roc-src

# 2. copy its compiler tree into our monorepo under compiler/
#    (review LICENSE — Roc is UPL-1.0, permissive)
rsync -a --exclude .git /tmp/roc-src/ compiler/

# 3. record provenance
echo "Forked from roc-lang/roc @ <commit-sha> on <date>" > compiler/UPSTREAM.md

# 4. add upstream remote for future cherry-picks (in a separate clone or as a note)
#    git remote add upstream https://github.com/roc-lang/roc.git
```

## Rename pass (Roc → Claw)

Global, mechanical, one PR:
- `roc` → `claw` (CLI binary, crate names, namespaces)
- `.roc` → `.claw` (file extension, everywhere it's referenced)
- stdlib module prefix `Roc*` → `Claw*` (or keep neutral names)
- docs/URLs → `clawlang.dev`
- Keep a `RENAME.md` mapping so upstream cherry-picks can be translated.

Do the rename **after** vendoring and **before** any feature work, so all subsequent diffs are clean.

## Cherry-pick workflow (ongoing)

```bash
# in a scratch clone of upstream, find the fix commit <sha>
# translate paths/names via RENAME.md, then apply as a normal patch to compiler/
git -C /tmp/roc-src show <sha> > /tmp/fix.patch
# hand-apply / adapt into compiler/, commit with "cherry-pick upstream <sha>: <desc>"
```

## Provenance & license hygiene
- `compiler/UPSTREAM.md` records the fork point SHA + date.
- Preserve Roc's LICENSE + attribution in `compiler/`.
- Decide Claw's own license before first public release (permissive recommended to match Roc + maximize adoption — the opposite of an adoption barrier).
