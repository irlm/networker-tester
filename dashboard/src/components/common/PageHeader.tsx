interface PageHeaderProps {
  title: string;
  subtitle?: string;
  action?: React.ReactNode;
}

export function PageHeader({ title, subtitle, action }: PageHeaderProps) {
  return (
    <div className="flex items-center justify-between mb-6 gap-2">
      <div>
        <h2 className="text-lg md:text-xl font-bold text-gray-100">{title}</h2>
        {subtitle && (
          <p className="text-sm text-gray-500 mt-1 hidden md:block">{subtitle}</p>
        )}
      </div>
      {action}
    </div>
  );
}
