'use client'

import { Suspense } from 'react'
import { useSearchParams } from 'next/navigation'

import { AppHeader } from '@/components/app-header'
import { Skeleton } from '@/components/ui/skeleton'
import { GatewayDetailContent } from '@/components/gateway/gateway-detail-content'

function GatewayDetailPageContent() {
  const searchParams = useSearchParams()
  const gatewayId = searchParams.get('id')

  return <GatewayDetailContent gatewayId={gatewayId} />
}

export default function GatewayDetailPage() {
  return (
    <Suspense
      fallback={
        <>
          <AppHeader
            breadcrumbs={[
              { label: 'Servers', href: '/gateways' },
              { label: 'Loading...' },
            ]}
          />
          <div className="flex-1 p-6">
            <div className="space-y-6">
              <div className="flex items-start justify-between">
                <div className="space-y-2">
                  <Skeleton className="h-8 w-48" />
                  <Skeleton className="h-5 w-32" />
                </div>
                <div className="flex gap-2">
                  <Skeleton className="h-9 w-20" />
                  <Skeleton className="h-9 w-20" />
                </div>
              </div>
              <Skeleton className="h-[400px] w-full rounded-lg" />
            </div>
          </div>
        </>
      }
    >
      <GatewayDetailPageContent />
    </Suspense>
  )
}
