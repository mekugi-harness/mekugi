import { type VariantProps, tv } from "tailwind-variants";

export const badgeVariants = tv(
  {
    base: "flex-none font-medium whitespace-nowrap",
    variants: {
      // chrome is the stock compact badge for page chrome; tag sits in text.
      size: {
        tag: "inline-flex h-[18px] items-center rounded-chip px-1.5 text-tiny leading-none",
        chrome:
          "inline-flex h-6 items-center gap-1.5 overflow-hidden rounded-control px-2 text-small leading-none",
      },
      variant: {
        default: "bg-neutral-tint text-neutral-ink",
        secondary: "bg-raised text-ink",
        destructive: "bg-negative-tint text-negative-ink",
        live: "bg-live-tint text-live-ink",
        working: "bg-working-tint text-working-ink",
        role: "bg-neutral-tint text-role",
        outline: "bg-raised text-ink",
        quiet: "bg-raised/80 text-muted",
        ghost: "text-muted",
        link: "text-ink underline-offset-4 hover:underline",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "tag",
    },
  },
  { twMerge: false },
);

export type BadgeVariant = VariantProps<typeof badgeVariants>["variant"];
export type BadgeSize = VariantProps<typeof badgeVariants>["size"];
