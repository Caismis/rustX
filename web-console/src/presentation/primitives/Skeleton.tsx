import clsx from 'clsx'
import { Skeleton as CossSkeleton } from '@/components/ui/skeleton'

const DOCUMENT_LINE_WIDTHS = ['54%', '96%', '88%', '76%', '94%', '62%', '84%', '46%'] as const

function SkeletonBar({
  width,
  height = 12,
  shape = 'bar',
}: {
  width: number | string
  height?: number
  shape?: 'bar' | 'pill' | 'control'
}) {
  return (
    <CossSkeleton
      className={clsx(shape === 'pill' && 'rounded-full', shape === 'control' && 'rounded-md')}
      style={{ width, height }}
      aria-hidden="true"
    />
  )
}

export function SettingsPageSkeleton({
  groups = 2,
  showHeaderActions = true,
  bodyOnly = false,
  lifecycle,
  label,
}: {
  groups?: number
  showHeaderActions?: boolean
  bodyOnly?: boolean
  lifecycle?: string
  label?: string
}) {
  const canvas = (
    <div className="flex flex-col gap-[24px] w-full max-w-[960px]">
      {Array.from({ length: groups }, (_, groupIndex) => (
        <div key={groupIndex} className="flex flex-col gap-[10px]">
          <SkeletonBar width={groupIndex === 0 ? 104 : 132} height={13} />
          <div className="overflow-hidden bg-[var(--settings-card-bg,var(--dsw-alias-bg-layer-1))] [border:1px_solid_var(--dsw-alias-border-l3)] rounded-[var(--dsw-radius-lg)]">
            <div className="flex items-center justify-between gap-[24px] px-[16px] py-[15px] [border-top:1px_solid_var(--dsw-alias-border-l3)] first:border-t-0">
              <div className="flex flex-auto flex-col gap-[7px] min-w-0">
                <SkeletonBar width={144} height={14} />
                <SkeletonBar width="64%" height={12} />
              </div>
              <SkeletonBar width={36} height={20} shape="pill" />
            </div>
            <div className="flex items-center justify-between gap-[24px] px-[16px] py-[15px] [border-top:1px_solid_var(--dsw-alias-border-l3)] first:border-t-0">
              <div className="flex flex-auto flex-col gap-[7px] min-w-0">
                <SkeletonBar width={128} height={14} />
                <SkeletonBar width="52%" height={12} />
              </div>
              <SkeletonBar width={148} height={32} shape="control" />
            </div>
            {groupIndex === 0 && (
              <div className="flex items-center justify-between gap-[24px] px-[16px] py-[15px] [border-top:1px_solid_var(--dsw-alias-border-l3)] first:border-t-0">
                <div className="flex flex-auto flex-col gap-[7px] min-w-0">
                  <SkeletonBar width={162} height={14} />
                  <SkeletonBar width="70%" height={12} />
                </div>
                <SkeletonBar width={96} height={32} shape="pill" />
              </div>
            )}
          </div>
        </div>
      ))}
    </div>
  )

  if (bodyOnly) {
    return (
      <div role="status" aria-busy="true" aria-label={label} data-lifecycle={lifecycle}>
        {canvas}
      </div>
    )
  }

  return (
    <div className="flex flex-col w-full h-full min-h-0 bg-[var(--dsw-alias-bg-base)]" role="status" aria-busy="true" aria-label={label} data-lifecycle={lifecycle}>
      <div className="flex items-start justify-between gap-[16px] pt-[24px] px-[32px] pb-[16px] max-[720px]:flex-col max-[720px]:pt-[20px] max-[720px]:px-[20px] max-[720px]:pb-[14px]">
        <div className="flex flex-col gap-[8px]">
          <SkeletonBar width={168} height={24} shape="control" />
          <SkeletonBar width={280} height={13} />
        </div>
        {showHeaderActions && (
          <div className="flex items-center gap-[8px]">
            <SkeletonBar width={184} height={32} shape="pill" />
            <SkeletonBar width={32} height={32} shape="pill" />
          </div>
        )}
      </div>
      <div className="flex-auto min-h-0 pt-[4px] px-[32px] pb-[32px] overflow-hidden max-[720px]:pt-[4px] max-[720px]:px-[20px] max-[720px]:pb-[24px]">{canvas}</div>
    </div>
  )
}

export function CatalogListSkeleton({
  rows = 4,
  label,
}: {
  rows?: number
  label?: string
}) {
  return (
    <div className="grid grid-cols-[repeat(auto-fill,minmax(320px,1fr))] max-[720px]:grid-cols-1 gap-[12px] w-full" role="status" aria-busy="true" aria-label={label}>
      {Array.from({ length: rows }, (_, index) => (
        <div key={index} className="flex gap-[12px] items-start px-[16px] py-[14px] bg-[var(--settings-card-bg,var(--dsw-alias-bg-layer-1))] [border:1px_solid_var(--dsw-alias-border-l3)] rounded-[var(--dsw-radius-xl,16px)]">
          <CossSkeleton className="shrink-0 w-[40px] h-[40px] rounded-[var(--dsw-radius-lg,12px)]" aria-hidden="true" />
          <div className="flex flex-auto flex-col gap-[8px] min-w-0">
            <div className="flex items-center justify-between gap-[12px]">
              <SkeletonBar width="46%" height={15} />
              <SkeletonBar width={44} height={22} shape="pill" />
            </div>
            <SkeletonBar width="88%" height={12} />
            <SkeletonBar width="60%" height={12} />
          </div>
        </div>
      ))}
    </div>
  )
}

export function DetailFormSkeleton({
  sections = 2,
  label,
}: {
  sections?: number
  label?: string
}) {
  return (
    <div className="flex flex-col gap-[20px] w-full" role="status" aria-busy="true" aria-label={label}>
      {Array.from({ length: sections }, (_, index) => (
        <div key={index} className="flex flex-col gap-[16px] px-[20px] py-[18px] bg-[var(--settings-card-bg,var(--dsw-alias-bg-layer-1))] [border:1px_solid_var(--dsw-alias-border-l3)] rounded-[var(--dsw-radius-xl,16px)]">
          <SkeletonBar width={index === 0 ? 112 : 140} height={15} />
          {index === 0 ? (
            <>
              <div className="grid grid-cols-2 max-[720px]:grid-cols-1 gap-[16px]">
                <div className="flex flex-col gap-[8px]">
                  <SkeletonBar width={64} height={12} />
                  <SkeletonBar width="100%" height={36} shape="control" />
                </div>
                <div className="flex flex-col gap-[8px]">
                  <SkeletonBar width={72} height={12} />
                  <SkeletonBar width="100%" height={36} shape="control" />
                </div>
              </div>
              <div className="flex flex-col gap-[8px]">
                <SkeletonBar width={88} height={12} />
                <SkeletonBar width="100%" height={84} shape="control" />
              </div>
            </>
          ) : (
            <>
              <div className="flex flex-col gap-[8px]">
                <SkeletonBar width={96} height={12} />
                <SkeletonBar width="100%" height={36} shape="control" />
              </div>
              <div className="flex flex-col gap-[8px]">
                <SkeletonBar width={118} height={12} />
                <SkeletonBar width="100%" height={36} shape="control" />
              </div>
            </>
          )}
        </div>
      ))}
    </div>
  )
}

export function DocumentLinesSkeleton({
  lines = 6,
  label,
}: {
  lines?: number
  label?: string
}) {
  return (
    <div className="flex flex-col gap-[12px] px-0 py-[4px]" role="status" aria-busy="true" aria-label={label}>
      {Array.from({ length: lines }, (_, index) => (
        <SkeletonBar
          key={index}
          width={DOCUMENT_LINE_WIDTHS[index % DOCUMENT_LINE_WIDTHS.length] ?? '72%'}
          height={index === 0 ? 18 : 13}
        />
      ))}
    </div>
  )
}
