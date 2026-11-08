# Released API compatibility

Numeric IDs and API compatibility are different checks. `abi-ids.lock` next to
the WIT keeps numeric identifiers stable. A baseline file (`--api-baseline`,
in the example `wit/api-baseline.snapshot`) records the
published types, tags, resource groups, method signatures (including parameter
names/order and sync/async), and host imports.

The lock file also records the wire shape each ID stands for (parameter and
result types of a method, the type of a field or case). If that shape changes,
the item gets a new ID and the old one is retired, so two sides built from
different contracts never decode each other's bytes by accident. That happens
unconditionally; whether the change is *allowed* is what the baseline decides.

The generator checks this baseline before writing outputs, including in
`--check` mode:

* Existing entries must remain unchanged within the same major version.
  Removing a method, field or case, changing its type/ID/signature, or changing
  sync/async requires a major version increase.
* New exported methods, types, enum/variant cases, and optional fields of an
  existing record require a minor version increase.
* New mandatory fields in an existing record, and new host imports/functions
  that existing apps would have to implement, require a major increase.
* Reordering and documentation changes do not change the snapshot.
* Versions cannot go backwards. The major-break rule also applies to 0.x
  packages so it matches the SDK loaders' existing compatibility rule.

Use the normal generation/check commands during development. At release time,
after reviewing the API diff, explicitly advance the baseline:

```sh
example/generate.sh --write-api-baseline
```

This is the only mode that writes the baseline; it cannot be combined with
`--check`. Commit the reviewed baseline alongside the version change. To
compare against a snapshot retrieved from a particular published release,
pass `--api-baseline /path/to/released.snapshot`.

The baseline of the example represents the contract as first published in this
repository; it does not claim that this version has already been distributed.
A regression test checks that the baseline is canonical and that the current
contract satisfies these compatibility rules.

Emitters flatten interfaces into each target package. Resources are associated
by WIT TypeId, while colliding generated type/member names are rejected
before outputs are written. This includes records, resources, error wrappers
and generated support classes.
