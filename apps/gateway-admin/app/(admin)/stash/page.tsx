import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { StashPageContent } from '@/components/stash/stash-page-content'

export default function Page() {
  return <><AppHeader breadcrumbs={[{ label: 'Workspace' }, { label: 'Stash' }]} /><div className={`${AURORA_PAGE_SHELL} flex-1`}><main className={`${AURORA_PAGE_FRAME} space-y-4`}><StashPageContent /></main></div></>
}
