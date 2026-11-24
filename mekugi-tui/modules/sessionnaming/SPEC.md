# Session naming

## Requirements

Inherited from [product requirements](../../../docs/product.md#user-requirements): support manual
session naming (`/title`) plus non-invasive automatic session naming for new untitled Main threads
that never overrides manual names or blocks conversation work.

## Current approach (replaceable)

- The module enables automatic naming for a new untitled Main thread. The
  app server takes up to 4,096 characters of its first nonempty user message.
  Children, side threads and resumed sessions do not trigger naming.
- After a successful model response, the app server samples a title once
  using the thread's provider and credentials. It requests `gpt-6-luna` at
  medium effort with a one-minute deadline. Usage counts toward the thread's
  total. Failure leaves the title unchanged; there is no fallback.
- The app server rechecks the stored name before saving a generated title.
  A manual name takes precedence. Removing the module disables automatic
  naming but leaves `/title` available.
