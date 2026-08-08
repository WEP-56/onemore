import * as React from "react"
import { Tooltip as TooltipPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

// Radix Tooltip has no imperative handle equivalent to base-ui's
// createHandle. Kept as a no-op factory so the export surface stays
// backward compatible for any consumer that references it.
const TooltipCreateHandle = () => ({})

// A zero hover delay makes every pointer crossing over chrome buttons and
// thread rows mount/unmount a body portal (forced reflow per crossing), which
// reads as whole-app jank. Uncontrolled tooltips must wait for a dwell.
const DEFAULT_TOOLTIP_DELAY_MS = 500
const TOOLTIP_POPUP_CLASS_NAME =
  "z-50 relative flex h-(--popup-height,auto) w-(--popup-width,auto) origin-(--transform-origin) text-balance rounded-md border bg-popover not-dark:bg-clip-padding text-popover-foreground text-xs shadow-md/5 px-(--viewport-inline-padding) py-1 [--viewport-inline-padding:--spacing(2)] transition-[width,height,scale,opacity] before:pointer-events-none before:absolute before:inset-0 before:rounded-[calc(var(--radius-md)-1px)] before:shadow-[0_1px_--theme(--color-black/4%)] data-ending-style:scale-98 data-starting-style:scale-98 data-ending-style:opacity-0 data-starting-style:opacity-0 data-instant:duration-0 dark:before:shadow-[0_-1px_--theme(--color-white/6%)]"

function TooltipProvider({
  delayDuration = DEFAULT_TOOLTIP_DELAY_MS,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Provider>) {
  return (
    <TooltipPrimitive.Provider
      data-slot="tooltip-provider"
      delayDuration={delayDuration}
      {...props}
    />
  )
}

function Tooltip({
  // `disabled` was a base-ui Root prop. Radix Root has no equivalent, so it
  // is accepted for backward compatibility and intentionally not forwarded
  // (consumers drive visibility through the controlled `open` prop).
  disabled: _disabled,
  // Optional per-tooltip dwell. Defaults to DEFAULT_TOOLTIP_DELAY_MS via
  // TooltipProvider; pass 0 for dense status chips that need near-instant tips.
  delayDuration,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Root> & {
  disabled?: boolean
  delayDuration?: number
}) {
  // Canonical shadcn wraps each Tooltip in its own Provider so consumers can
  // render <Tooltip> standalone (Radix Root throws without a Provider ancestor,
  // unlike the previous base-ui implementation).
  return (
    <TooltipProvider delayDuration={delayDuration}>
      <TooltipPrimitive.Root data-slot="tooltip" {...props} />
    </TooltipProvider>
  )
}

function TooltipTrigger({
  delay: _delay,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Trigger> & { delay?: number }) {
  return <TooltipPrimitive.Trigger data-slot="tooltip-trigger" {...props} />
}

function TooltipPopup({
  className,
  align = "center",
  sideOffset = 4,
  side = "top",
  // `anchor` was a base-ui Positioner prop. Radix Tooltip has no anchor
  // concept; accepted for backward compatibility and not forwarded.
  anchor: _anchor,
  children,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Content> & {
  anchor?: unknown
}) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        align={align}
        side={side}
        sideOffset={sideOffset}
        className={cn(TOOLTIP_POPUP_CLASS_NAME, className)}
        data-slot="tooltip-popup"
        {...props}
      >
        {children}
      </TooltipPrimitive.Content>
    </TooltipPrimitive.Portal>
  )
}

export {
  TooltipCreateHandle,
  TOOLTIP_POPUP_CLASS_NAME,
  TooltipProvider,
  Tooltip,
  TooltipTrigger,
  TooltipPopup,
  TooltipPopup as TooltipContent,
}
