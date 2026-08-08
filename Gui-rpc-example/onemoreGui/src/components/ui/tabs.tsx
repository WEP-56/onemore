import * as React from "react"
import { Tabs as TabsPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

type TabsVariant = "default" | "underline";

function Tabs({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Root>) {
  return (
    <TabsPrimitive.Root
      className={cn(
        "flex flex-col gap-2 data-[orientation=vertical]:flex-row",
        className,
      )}
      data-slot="tabs"
      {...props}
    />
  );
}

function TabsList({
  variant = "default",
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.List> & {
  variant?: TabsVariant;
}) {
  return (
    <TabsPrimitive.List
      className={cn(
        "relative z-0 flex w-fit items-center justify-center gap-1 text-muted-foreground",
        "data-[orientation=vertical]:flex-col",
        variant === "default"
          ? "data-[orientation=horizontal]:w-full data-[orientation=horizontal]:justify-start data-[orientation=horizontal]:gap-0 data-[orientation=horizontal]:border-b data-[orientation=horizontal]:border-border data-[orientation=horizontal]:py-1"
          : "data-[orientation=vertical]:px-1 data-[orientation=horizontal]:w-full data-[orientation=horizontal]:justify-start data-[orientation=horizontal]:gap-0 data-[orientation=horizontal]:border-b data-[orientation=horizontal]:border-border data-[orientation=horizontal]:py-1",
        className,
      )}
      data-slot="tabs-list"
      {...props}
    />
  );
}

function TabsTab({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Trigger>) {
  return (
    <TabsPrimitive.Trigger
      className={cn(
        "[&_svg]:-mx-0.5 relative flex h-9 shrink-0 grow cursor-pointer items-center justify-center gap-1.5 whitespace-nowrap border-0 rounded-none bg-transparent px-3 font-medium text-base outline-none shadow-none transition-[color,background-color] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring data-disabled:pointer-events-none data-[orientation=vertical]:w-full data-[orientation=vertical]:justify-start data-[selected]:text-foreground data-[state=active]:text-foreground data-active:text-foreground data-disabled:opacity-64 sm:h-8 sm:text-sm [&_svg:not([class*='size-'])]:size-4.5 sm:[&_svg:not([class*='size-'])]:size-4 [&_svg]:pointer-events-none [&_svg]:shrink-0",
        "after:absolute after:z-10 after:bg-primary after:opacity-0 after:transition-opacity after:content-[''] data-[state=active]:after:opacity-100 data-[orientation=horizontal]:after:inset-x-0 data-[orientation=horizontal]:after:bottom-0 data-[orientation=horizontal]:after:h-0.5 data-[orientation=vertical]:after:inset-y-0 data-[orientation=vertical]:after:left-0 data-[orientation=vertical]:after:w-0.5",
        className,
      )}
      data-slot="tabs-tab"
      {...props}
    />
  );
}

function TabsPanel({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Content>) {
  return (
    <TabsPrimitive.Content
      className={cn("flex-1 outline-none", className)}
      data-slot="tabs-content"
      {...props}
    />
  );
}

export {
  Tabs,
  TabsList,
  TabsTab,
  TabsTab as TabsTrigger,
  TabsPanel,
  TabsPanel as TabsContent,
};
