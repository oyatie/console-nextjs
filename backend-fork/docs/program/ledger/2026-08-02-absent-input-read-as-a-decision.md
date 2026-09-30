# 2026-08-02 — absent input read as a decision

`deploy/opentofu/variables.tf` gave `talos_image_ocid` a `default = ""`, and the Talos node was
declared under `count = var.talos_image_ocid != "" ? 1 : 0`. The default exists for a real reason:
the OCI qcow2 import corrupts Talos's GPT, so the first apply had to stand up network, storage and
bastion *before* a bootable image OCID existed. That bootstrap ran once, on 2026-06-22, and can
never run again — the node it produced is irreplaceable.

What outlived the bootstrap was the default. Measured on the branch, plan only, no apply:

| `talos_image_ocid` | result | `oci_core_instance.node` in the plan |
|---|---|---|
| `""` — the old default | `Plan: 9 to add, 0 to change, 0 to destroy` | **0 occurrences** |
| unset — after this change | `Error: No value for required variable`, `variables.tf:55` | plan refuses to run |

The top row is the whole finding. It is not an error, not a warning, and not an empty plan: it is a
clean nine-resource plan that an operator would reasonably apply, and it does not mention the
cluster's only instance anywhere. A missing or mistyped tfvars file was indistinguishable from a
deliberate request for no node.

**Absent input is not a decision.** A sentinel default converts one into the other, and it does so
most quietly in exactly the case where the input went missing by accident. The fix is to delete the
default, not to add a guard — a guard catches the symptom and leaves `""` a legal way to ask for
nothing.

`prevent_destroy = true` is the second half, and it was proven rather than read. The live node was
imported into a throwaway local state, `talos_image_ocid` was set empty so the count went 1 → 0, and
the plan returned `Resource instance cannot be destroyed`. The state file was deleted; nothing was
applied and no OCI resource was mutated. This is the distinction the program keeps paying for: a
`lifecycle` block is a claim until something has been observed refusing.

The second A1 in the same module — `oci_core_instance.flasher`, the one-shot dd-flash helper — was
deleted rather than set to `count = 0`. It was unconditional and sized from the same
`node_ocpus` / `node_memory_gbs` defaults, so an apply would have tried to create a second
4 OCPU / 24 GB instance against an allotment `deploy/OPS-RUNBOOK.md:42-43` records as entirely spent
by the node. `count = 0` is the smaller diff and the worse one: a disabled resource reads as
temporarily off, and the next person to want a bastion flips it back. Deleting it also retired its
image data source, its `ssh_public_key` variable and its two outputs, all dead the moment it was.

The prose was corrected in the same change, because three files described a flasher that no longer
exists and a Talos node that no longer needs one. This repo's recurring failure is prose asserting
unimplemented intent; the inverse — prose describing implementation that has been removed — is the
same defect from the other side.

Every capability, evidence contract, jurisdiction binding, Korea control, review disposition, and
exposure state remains `HOLD`.
