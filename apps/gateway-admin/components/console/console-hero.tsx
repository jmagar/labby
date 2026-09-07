'use client'

import * as React from 'react'

/**
 * The console's page hero, measured off the rendered Gateway Console mock.
 *
 * Every top-level screen in the mock opens the same way: an uppercase eyebrow
 * with an optional live pulse chip, a 30px display title, an action cluster on
 * the right, and a stat strip welded to the card's bottom edge rather than
 * floating as separate cards.
 *
 * Overview and Gateway build their strips inline because they carry extra
 * furniture (fleet-health squares, grouped exposure bars). Screens with a
 * plain row of stats should pass `stats` and let this draw it.
 */

export type ConsoleHeroStat = {
  label: string
  value: React.ReactNode
  icon?: React.ReactNode
  /** Value colour; defaults to primary text, matching the mock. */
  tone?: string
}

export function ConsoleHero({
  eyebrow,
  pulse,
  title,
  description,
  actions,
  stats,
  children,
}: {
  eyebrow: string
  pulse?: { color: string; label?: string }
  title: string
  description?: React.ReactNode
  actions?: React.ReactNode
  stats?: ConsoleHeroStat[]
  /** Custom strip content, when `stats` is not expressive enough. */
  children?: React.ReactNode
}) {
  return (
    <div
      style={{
        borderRadius: 'var(--radius-3)',
        border:
          '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
        background:
          'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))',
        boxShadow: 'var(--aurora-shadow-strong), inset 0 1px 0 rgba(255,255,255,0.05)',
      }}
    >
      <div
        data-console-hero-main="1"
        style={{
          display: 'flex',
          alignItems: 'flex-end',
          justifyContent: 'space-between',
          gap: 16,
          padding: '22px 24px 18px',
          flexWrap: 'wrap',
        }}
      >
        <div data-console-hero-copy="1" style={{ minWidth: 0 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}>
            <span
              style={{
                fontSize: 10.5,
                fontWeight: 700,
                letterSpacing: '0.16em',
                textTransform: 'uppercase',
                color: 'var(--aurora-text-muted)',
              }}
            >
              {eyebrow}
            </span>
            {pulse ? (
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                <span
                  style={{
                    width: 6,
                    height: 6,
                    borderRadius: 999,
                    background: pulse.color,
                    boxShadow: `0 0 4px ${pulse.color}`,
                    animation: 'ovPulse 2.4s ease-in-out infinite',
                  }}
                />
                {pulse.label ? <span style={{ fontSize: 10.5, fontWeight: 650, color: pulse.color }}>
                  {pulse.label}
                </span> : null}
              </span>
            ) : null}
          </div>
          <h1
            data-console-hero-title="1"
            style={{
              margin: '8px 0 0',
              fontFamily: 'var(--font-display)',
              fontSize: 30,
              lineHeight: 1.04,
              fontWeight: 800,
              color: 'var(--aurora-text-primary)',
              whiteSpace: 'nowrap',
            }}
          >
            {title}
          </h1>
          {description ? (
            <div style={{ marginTop: 8, maxWidth: 660, fontSize: 12.5, lineHeight: 1.55, color: 'var(--aurora-text-muted)' }}>
              {description}
            </div>
          ) : null}
        </div>

        {actions ? (
          <div data-console-hero-actions="1" style={{ flexShrink: 0, display: 'flex', alignItems: 'center', gap: 6 }}>
            {actions}
          </div>
        ) : null}
      </div>

      {stats || children ? (
        <div
          style={{
            padding: '12px 14px',
            borderTop:
              '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
            background: 'var(--gw0-0_28)',
            borderRadius: '0 0 var(--radius-3) var(--radius-3)',
          }}
        >
          {stats ? (
            <div
              data-mobile-grid2="1"
              style={{
                display: 'grid',
                gridTemplateColumns: 'repeat(auto-fit, minmax(118px, 1fr))',
                gap: '8px 0',
              }}
            >
              {stats.map((stat, index) => (
                <div
                  key={stat.label}
                  title={stat.label}
                  data-console-hero-stat={stat.label}
                  style={{
                    minWidth: 0,
                    padding: '2px 12px',
                    borderRight:
                      index === stats.length - 1
                        ? undefined
                        : '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
                  }}
                >
                  <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                    {stat.icon ? (
                      <span
                        style={{
                          flexShrink: 0,
                          color: 'var(--aurora-text-muted)',
                          display: 'grid',
                        }}
                      >
                        {stat.icon}
                      </span>
                    ) : null}
                    <span
                      style={{
                        fontSize: 9.5,
                        fontWeight: 700,
                        letterSpacing: '0.08em',
                        textTransform: 'uppercase',
                        color: 'var(--aurora-text-muted)',
                        whiteSpace: 'nowrap',
                      }}
                    >
                      {stat.label}
                    </span>
                  </div>
                  <div
                    data-console-hero-stat-value="1"
                    style={{
                      marginTop: 6,
                      fontFamily: 'var(--font-display)',
                      fontSize: 21,
                      lineHeight: 1,
                      fontWeight: 800,
                      fontVariantNumeric: 'tabular-nums',
                      color: stat.tone ?? 'var(--aurora-text-primary)',
                    }}
                  >
                    {stat.value}
                  </div>
                </div>
              ))}
            </div>
          ) : null}
          {children}
        </div>
      ) : null}
    </div>
  )
}
