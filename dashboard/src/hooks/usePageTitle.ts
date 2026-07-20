import { useEffect } from 'react';
import { PRODUCT_NAME } from '../lib/brand';

export function usePageTitle(title: string) {
  useEffect(() => {
    const prev = document.title;
    document.title = `${title} | ${PRODUCT_NAME}`;
    return () => {
      document.title = prev;
    };
  }, [title]);
}
