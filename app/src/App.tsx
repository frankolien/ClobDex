import { Header } from "./components/Header.tsx";
import { Markets } from "./components/Markets.tsx";
import { Portfolio } from "./components/Portfolio.tsx";
import { Trade } from "./components/Trade.tsx";
import { WalletProvider } from "./components/Wallet.tsx";
import { useRoute } from "./lib/router.ts";
import "./app.css";

export function App() {
  const route = useRoute();

  return (
    <WalletProvider>
      <div className="app">
        <Header />
        <main>
          {route.name === "markets" && <Markets />}
          {route.name === "trade" && <Trade market={route.market} />}
          {route.name === "portfolio" && <Portfolio />}
        </main>
      </div>
    </WalletProvider>
  );
}
