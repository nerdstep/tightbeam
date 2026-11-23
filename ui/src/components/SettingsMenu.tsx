import { Settings2Icon } from 'lucide-react'
import { useEffect, useState } from 'react'

import { api, type Prefs } from '@/api'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { run } from '@/store'

/** The gear menu in the header. `onUnpair` adds an Unpair item (chat screen only). */
export function SettingsMenu({ onUnpair }: { onUnpair?: () => void }) {
  const [prefs, setPrefs] = useState<Prefs | null>(null)
  const [version, setVersion] = useState('')

  useEffect(() => {
    void run(api.getPrefs()).then((p) => p && setPrefs(p))
    void run(api.version()).then((v) => v && setVersion(v))
  }, [])

  const update = (change: Partial<Prefs>, save: Promise<void>) => {
    setPrefs((p) => p && { ...p, ...change })
    // Put it back if saving failed (the error shows in the notice banner).
    void run(save.then(() => true)).then(
      (ok) => !ok && void run(api.getPrefs()).then((p) => p && setPrefs(p)),
    )
  }

  return (
    <DropdownMenu>
      <DropdownMenuTrigger render={<Button variant="ghost" size="icon-sm" aria-label="Settings" />}>
        <Settings2Icon />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-52">
        <DropdownMenuCheckboxItem
          checked={prefs?.notifications ?? false}
          disabled={!prefs}
          onCheckedChange={(on) => update({ notifications: on }, api.setNotifications(on))}
        >
          Notifications
        </DropdownMenuCheckboxItem>
        {prefs?.start_at_login != null && (
          <DropdownMenuCheckboxItem
            checked={prefs.start_at_login}
            onCheckedChange={(on) => update({ start_at_login: on }, api.setStartAtLogin(on))}
          >
            Start at login
          </DropdownMenuCheckboxItem>
        )}
        {onUnpair && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem variant="destructive" onClick={onUnpair}>
              Unpair…
            </DropdownMenuItem>
          </>
        )}
        {version && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuGroup>
              <DropdownMenuLabel className="text-xs font-normal">
                Tightbeam {version}
              </DropdownMenuLabel>
            </DropdownMenuGroup>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
