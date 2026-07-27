# Repository cutover

This is a one-time authority change from the existing TrueNAS repository tree
to `/home/sulion/repos` on `sulion-enclave`. It is not bidirectional sync.

Set the exact source and host names for the current installation:

```bash
export OLD_REPOS=/mnt/apps/apps/sulion/repos
export ENCLAVE=sulion@sulion-enclave
```

Run the copy from a TrueNAS shell. The source and destination must both name
the canonical repository roots. The trailing slashes matter:

```bash
sudo rsync -aHAX --numeric-ids --info=progress2 \
  "${OLD_REPOS}/" "${ENCLAVE}:/home/sulion/repos/"
```

The single-user source files must already be owned by UID/GID 7321 so the
remote `sulion` process can preserve their owner, ACLs, and user xattrs. If the
old tree contains other owners or root-only xattrs, stop: copy it through an
explicit root-capable transfer path and do not silently discard that metadata.

## Compare before cutover

Compare repository names and Git refs:

```bash
sudo find "${OLD_REPOS}" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort
ssh "${ENCLAVE}" \
  "find /home/sulion/repos -mindepth 1 -maxdepth 1 -type d -printf '%f\\n' | sort"
```

For each repository, compare the complete ref set:

```bash
sudo git -C "${OLD_REPOS}/REPO" show-ref | sort
ssh "${ENCLAVE}" "git -C /home/sulion/repos/REPO show-ref | sort"
```

Compare a deterministic file-content inventory on both machines:

```bash
sudo bash -c \
  'cd "$1" && find . -type f -print0 | sort -z | xargs -0 sha256sum' \
  _ "${OLD_REPOS}"
ssh "${ENCLAVE}" \
  "cd /home/sulion/repos && find . -type f -print0 | sort -z | xargs -0 sha256sum"
```

The output must match. Also inspect representative repository roots and
SMB-created directories:

```bash
sudo getfacl -p "${OLD_REPOS}/REPO"
sudo getfattr -d -m- "${OLD_REPOS}/REPO"
ssh "${ENCLAVE}" \
  "getfacl -p /home/sulion/repos/REPO; getfattr -d -m- /home/sulion/repos/REPO"
```

Do not proceed if Git refs, content, effective ACLs, or required xattrs differ.

## Final authority switch

1. Stop creation of PTYs and repository mutations on the old Sulion runtime.
2. Run the same `rsync` command again with `--delete`.
3. Repeat the comparisons.
4. Make the TrueNAS repository tree read-only.
5. Start the dedicated node stack.
6. Connect Windows and macOS clients to `sulion-enclave` and verify a real
   create, rename, and delete.
7. Verify one browser PTY, Git status, code intelligence, transcript ingest,
   secret redemption, Docker, and the expected Supabase stack.

Never leave both copies writable.

Before any new NixOS writes, rollback may simply restore the old deployment and
SMB endpoint. After new writes, quiesce both sides, copy the NixOS changes back,
and repeat the same comparisons before restoring TrueNAS authority.
