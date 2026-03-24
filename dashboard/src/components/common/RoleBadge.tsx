const ROLE_COLORS: Record<string, string> = {
  admin: 'text-green-400',
  operator: 'text-cyan-400',
  viewer: 'text-gray-500',
};

interface RoleBadgeProps {
  role: string;
  className?: string;
}

export function RoleBadge({ role, className = 'text-[10px]' }: RoleBadgeProps) {
  return (
    <span className={`${className} ${ROLE_COLORS[role] || 'text-gray-500'}`}>
      {role}
    </span>
  );
}
