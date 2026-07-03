import type { NavigateFunction } from 'react-router-dom';
import { Unauthorized } from '../api';

// withAuth: wraps any async API call and redirects to /admin/login on Unauthorized.
//
// Chosen over React error boundary for one reason: admin API calls happen in
// useEffect hooks and event handlers — not during render — so error boundaries
// can't catch them. A catch-and-redirect helper applied at the call site is
// the only mechanism that works uniformly across both paths. All admin
// components MUST use this wrapper for every api call so the guard is consistent.
export function withAuth<T>(fn: () => Promise<T>, navigate: NavigateFunction): Promise<T> {
  return fn().catch((err: unknown) => {
    if (err instanceof Unauthorized) {
      navigate('/admin/login', { replace: true });
      // Never resolves — navigation has already fired; halting here prevents
      // partial state updates in the calling component.
      return new Promise<T>(() => {});
    }
    throw err;
  });
}
