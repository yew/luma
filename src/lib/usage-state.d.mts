type AuthState = { state: string; error?: { code: string } | null; snapshot?: { fetched_at: number } | null } | null;
export function usageState(auth: AuthState, options?: { errorCode?: string; hasError?: boolean; signingIn?: boolean; busy?: boolean; now?: number; refreshIntervalSeconds?: number }): { label: string; symbol: string; kind: string };
export function refreshWaitSeconds(deadline: number | null | undefined, now: number): number;
