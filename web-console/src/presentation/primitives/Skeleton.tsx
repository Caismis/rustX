import clsx from 'clsx'
import css from './Skeleton.module.css'

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
    <span
      className={clsx(css.block, shape === 'pill' && css.pill, shape === 'control' && css.control)}
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
    <div className={css.canvas}>
      {Array.from({ length: groups }, (_, groupIndex) => (
        <div key={groupIndex} className={css.group}>
          <SkeletonBar width={groupIndex === 0 ? 104 : 132} height={13} />
          <div className={css.groupCard}>
            <div className={css.row}>
              <div className={css.rowCopy}>
                <SkeletonBar width={144} height={14} />
                <SkeletonBar width="64%" height={12} />
              </div>
              <SkeletonBar width={36} height={20} shape="pill" />
            </div>
            <div className={css.row}>
              <div className={css.rowCopy}>
                <SkeletonBar width={128} height={14} />
                <SkeletonBar width="52%" height={12} />
              </div>
              <SkeletonBar width={148} height={32} shape="control" />
            </div>
            {groupIndex === 0 && (
              <div className={css.row}>
                <div className={css.rowCopy}>
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
    <div className={css.page} role="status" aria-busy="true" aria-label={label} data-lifecycle={lifecycle}>
      <div className={css.pageHead}>
        <div className={css.pageHeadCopy}>
          <SkeletonBar width={168} height={24} shape="control" />
          <SkeletonBar width={280} height={13} />
        </div>
        {showHeaderActions && (
          <div className={css.pageHeadTools}>
            <SkeletonBar width={184} height={32} shape="pill" />
            <SkeletonBar width={32} height={32} shape="pill" />
          </div>
        )}
      </div>
      <div className={css.pageBody}>{canvas}</div>
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
    <div className={css.catalogList} role="status" aria-busy="true" aria-label={label}>
      {Array.from({ length: rows }, (_, index) => (
        <div key={index} className={css.catalogCard}>
          <span className={clsx(css.block, css.tile)} aria-hidden="true" />
          <div className={css.catalogMain}>
            <div className={css.catalogTop}>
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
    <div className={css.formStack} role="status" aria-busy="true" aria-label={label}>
      {Array.from({ length: sections }, (_, index) => (
        <div key={index} className={css.formCard}>
          <SkeletonBar width={index === 0 ? 112 : 140} height={15} />
          {index === 0 ? (
            <>
              <div className={css.formGrid}>
                <div className={css.field}>
                  <SkeletonBar width={64} height={12} />
                  <SkeletonBar width="100%" height={36} shape="control" />
                </div>
                <div className={css.field}>
                  <SkeletonBar width={72} height={12} />
                  <SkeletonBar width="100%" height={36} shape="control" />
                </div>
              </div>
              <div className={css.field}>
                <SkeletonBar width={88} height={12} />
                <SkeletonBar width="100%" height={84} shape="control" />
              </div>
            </>
          ) : (
            <>
              <div className={css.field}>
                <SkeletonBar width={96} height={12} />
                <SkeletonBar width="100%" height={36} shape="control" />
              </div>
              <div className={css.field}>
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
    <div className={css.lines} role="status" aria-busy="true" aria-label={label}>
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
