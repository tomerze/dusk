import { useMe } from './queries'

export interface Permissions {
  canOperate: boolean
  canAdminister: boolean
}

export function usePermissions(): Permissions {
  const me = useMe()
  const role = me.data?.role
  return {
    canOperate: role === 'operator' || role === 'admin',
    canAdminister: role === 'admin',
  }
}

export const operatorOnly =
  'Your role can view but not change this. Ask an admin for the operator role.'
