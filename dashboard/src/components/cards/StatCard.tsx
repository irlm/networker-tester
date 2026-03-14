interface StatCardProps {
  label: string;
  value: number | string;
  accent?: string;
}

export function StatCard({ label, value, accent = 'text-cyan-400' }: StatCardProps) {
  return (
    <div className="bg-[#12131a] border border-gray-800 rounded-lg p-4">
      <p className="text-xs text-gray-500 uppercase tracking-wider mb-1">{label}</p>
      <p className={`text-2xl font-bold ${accent}`}>{value}</p>
    </div>
  );
}
