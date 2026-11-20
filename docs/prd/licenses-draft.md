# Licenses and paid add-ons (draft, not ready)

Draft that came out of the review of the [Add-on platform](addon-platform.md) PRD (phase 7, finding 2): these parts
were removed from the platform because they depend on decisions that are still open and contradicted each other.
It will become a PRD when the decisions are made.

## Decisions to make before writing the PRD

- Which add-ons are paid.
- Selling individual add-ons, bundles, or both.
- Lifetime license or subscription; how many PCs per license.
- Vendor (Lemon Squeezy, Paddle or other).
- What users who today use a page for free see when it becomes paid (acquired rights, automatic trial, other).

## Contradictions to resolve (from the review)

- **Offline versus PC count.** A license verified only on the PC cannot know on how many PCs it is active:
  counting and moving activations requires contacting the vendor with an instance identifier.
  A choice must be made: (a) online activation with the vendor's API and the result saved locally, with no server
  of our own; or (b) Ed25519-signed licenses, which require a service of our own that signs each purchase
  (webhook + function), with the private key to be guarded and rotated.
- **Privacy.** "No identifier sent" must be rewritten as "only a random instance id, generated at activation, never
  hardware or usage data", and it must list which operations are online.
- **Trial.** It must say how it starts (a local counter, which can be bypassed, or a trial license from the vendor),
  how long it lasts, where it is saved, and whether it can be repeated. The accepted risk must be stated.
- **Purchase recovery.** It requires the vendor's portal: at most a link to the customer portal.
- **License format.** Format version and key rotation in case of compromise.
- **Known limit.** As long as the code of paid add-ons is in the free executable, anyone who modifies the binary
  bypasses the check.

## Stories removed from the platform (to be reviewed)

1. As a user, I want to see the license status of each add-on (Free, Trial, Purchased, Not purchased).
2. As a user, I want to enter a license key from Settings › Add-ons, so that purchased add-ons are unlocked.
3. As a user, I want a valid license to keep working offline after activation.
4. As a user, I want a distinct message for a key that is invalid, expired, or for another product, with the action I can take.
5. As a user, I want an add-on that is not purchased to show a locked preview with a purchase button.
6. As a user, I want an add-on that is not purchased to start no work.
7. As a user, I want to try a paid add-on for a period and see how many days I have left.
8. As a user, I want the saved data (recordings, histories) to remain on disk when the trial ends.
9. As a user, I want to see what I have purchased and on how many PCs it is active.
10. As a user, I want to move a license to another PC.
11. As a user, I want a link to recover a purchase.
12. As a user, I want payment to take place on the store's website, outside the app.
13. As a user, I want the app to send only a random instance id for the license, never usage or hardware data.
14. As a developer, I want no one to be able to generate licenses from the executable.
15. As a developer, I want to be able to sell individual add-ons or bundles.

The full state table is also needed: on/off × Free / Trial / Purchased / Not purchased /
Trial expired, with the menu, page, panel, `--page` and work for each case.
