import * as React from "react"

import { cn } from "@/lib/utils"

function Field({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="field"
      className={cn("flex flex-col items-start gap-2", className)}
      {...props}
    />
  )
}

function FieldLabel({ className, ...props }: React.ComponentProps<"label">) {
  return (
    <label
      data-slot="field-label"
      className={cn(
        "inline-flex items-center gap-2 font-medium text-base/4.5 text-foreground sm:text-sm/4",
        className
      )}
      {...props}
    />
  )
}

function FieldItem({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div data-slot="field-item" className={cn("flex", className)} {...props} />
  )
}

function FieldDescription({ className, ...props }: React.ComponentProps<"p">) {
  return (
    <p
      data-slot="field-description"
      className={cn("text-muted-foreground text-xs", className)}
      {...props}
    />
  )
}

function FieldError({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      role="alert"
      data-slot="field-error"
      className={cn("text-destructive-foreground text-xs", className)}
      {...props}
    />
  )
}

function FieldControl({ className, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      data-slot="field-control"
      className={cn(className)}
      {...props}
    />
  )
}

function FieldValidity({
  children,
}: {
  children: (state: { valid: boolean | null }) => React.ReactNode
}) {
  return <>{children({ valid: null })}</>
}

export {
  Field,
  FieldLabel,
  FieldDescription,
  FieldError,
  FieldControl,
  FieldItem,
  FieldValidity,
}
