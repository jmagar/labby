'use client'

import * as React from 'react'
import Link from 'next/link'
import { LabbyIcon } from '@/components/labby-icon'
import { usePathname } from 'next/navigation'
import {
  Cable,
  MessageSquareText,
  LayoutDashboard,
  ShoppingBag,
  Settings,
  Activity,
  ScrollText,
  HelpCircle,
  WandSparkles,
  Server,
} from 'lucide-react'

import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
} from '@/components/ui/sidebar'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import { ThemeToggle } from '@/components/theme-toggle'
import {
  sessionAvatarFallback,
  sessionPrimaryEmail,
} from '@/lib/auth/session-presenter'
import { logoutBrowserSession, useBrowserSession } from '@/lib/auth/session'

export const primarySidebarNavigation = [
  {
    title: 'Overview',
    url: '/',
    icon: LayoutDashboard,
  },
  {
    title: 'Servers',
    url: '/gateways',
    icon: Cable,
  },
  {
    title: 'Nodes',
    url: '/nodes',
    icon: Server,
  },
  {
    title: 'Marketplace',
    url: '/marketplace',
    icon: ShoppingBag,
  },
  {
    title: 'Chat',
    url: '/chat',
    icon: MessageSquareText,
  },
  {
    title: 'Setup',
    url: '/setup',
    icon: WandSparkles,
  },
  {
    title: 'Activity',
    url: '/activity',
    icon: Activity,
  },
  {
    title: 'Logs',
    url: '/logs',
    icon: ScrollText,
  },
]

export const secondarySidebarNavigation = [
  {
    title: 'Settings',
    url: '/settings',
    icon: Settings,
  },
  {
    title: 'Documentation',
    url: '/docs',
    icon: HelpCircle,
  },
]

export function AppSidebar() {
  const pathname = usePathname()
  const session = useBrowserSession()

  const isActive = (url: string) => {
    if (url === '/') return pathname === '/'
    return pathname.startsWith(url)
  }

  return (
    <Sidebar collapsible="icon">
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg" asChild>
              <Link href="/">
                <LabbyIcon size={32} />
                <div className="grid flex-1 text-left text-sm leading-tight">
                  <span className="truncate font-bold text-sidebar-foreground">Labby</span>
                  <span className="truncate text-xs text-sidebar-foreground/70">Admin Console</span>
                </div>
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>

      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupLabel>Navigation</SidebarGroupLabel>
          <SidebarGroupContent>
            <SidebarMenu>
              {primarySidebarNavigation.map((item) => (
                <SidebarMenuItem key={item.title}>
                  <SidebarMenuButton asChild isActive={isActive(item.url)} tooltip={item.title}>
                    <Link href={item.url}>
                      <item.icon />
                      <span>{item.title}</span>
                    </Link>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              ))}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>

        <SidebarGroup className="mt-auto">
          <SidebarGroupLabel>Support</SidebarGroupLabel>
          <SidebarGroupContent>
            <SidebarMenu>
              {secondarySidebarNavigation.map((item) => (
                <SidebarMenuItem key={item.title}>
                  <SidebarMenuButton asChild isActive={isActive(item.url)} tooltip={item.title}>
                    <Link href={item.url}>
                      <item.icon />
                      <span>{item.title}</span>
                    </Link>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              ))}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>

      <SidebarFooter>
        <SidebarMenu>
          {session.status === 'authenticated' ? (
            <SidebarMenuItem>
              <div className="rounded-lg border border-sidebar-border/70 bg-sidebar-accent/40 px-2 py-2 group-data-[collapsible=icon]:hidden">
                <div className="flex items-center gap-3">
                  <Avatar className="size-9 border border-sidebar-border/60">
                    <AvatarFallback className="bg-sidebar-primary text-sidebar-primary-foreground text-xs font-semibold">
                      {sessionAvatarFallback(session.user)}
                    </AvatarFallback>
                  </Avatar>
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-xs font-medium uppercase tracking-[0.18em] text-sidebar-foreground/55">
                      Signed In
                    </p>
                    <p className="truncate text-sm font-medium text-sidebar-foreground">
                      {sessionPrimaryEmail(session.user)}
                    </p>
                  </div>
                </div>
                <button
                  className="mt-3 text-xs text-aurora-text-muted transition hover:text-aurora-text-primary"
                  onClick={() => {
                    void logoutBrowserSession()
                  }}
                  type="button"
                >
                  Sign out
                </button>
              </div>
            </SidebarMenuItem>
          ) : null}
          <SidebarMenuItem>
            <div className="flex items-center justify-between px-2 py-1">
              <span className="text-xs text-aurora-text-muted group-data-[collapsible=icon]:hidden">
                Theme
              </span>
              <ThemeToggle />
            </div>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>

      <SidebarRail />
    </Sidebar>
  )
}
