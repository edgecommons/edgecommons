import { useEffect, useState } from "react";
import {
  Theme, Header, HeaderName, Dropdown, Tabs, TabList, Tab, TabPanels, TabPanel,
  Loading, InlineNotification, Tag,
} from "@carbon/react";
import { api, type DefinitionView } from "./api";
import { ConfigLayers } from "./ConfigLayers";
import { RenderReview } from "./RenderReview";

export function App() {
  const [def, setDef] = useState<DefinitionView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [profile, setProfile] = useState<string>("");

  useEffect(() => {
    api.definition()
      .then((d) => { setDef(d); if (d.profiles[0]) setProfile(d.profiles[0].name); })
      .catch((e: Error) => setError(e.message));
  }, []);

  if (error) {
    return (
      <Theme theme="g100">
        <div style={{ padding: "2rem" }}>
          <InlineNotification kind="error" title="Cannot reach the Studio server" subtitle={error} hideCloseButton />
        </div>
      </Theme>
    );
  }
  if (!def) return <Theme theme="g100"><Loading /></Theme>;

  return (
    <Theme theme="g100">
      <Header aria-label="Deployment Studio">
        <HeaderName href="#" prefix="EdgeCommons">Deployment Studio</HeaderName>
      </Header>
      <main style={{ padding: "3rem 2rem 2rem", maxWidth: "72rem", margin: "0 auto" }}>
        <div style={{ display: "flex", alignItems: "baseline", gap: "1rem", marginBottom: "0.25rem" }}>
          <h1 style={{ fontWeight: 300, margin: 0 }}>{def.name}</h1>
          <Tag type="cool-gray">read-only</Tag>
        </div>
        {def.description && <p style={{ color: "#c6c6c6", marginTop: 0 }}>{def.description}</p>}
        <div style={{ maxWidth: "22rem", margin: "1.5rem 0" }}>
          <Dropdown
            id="profile"
            titleText="Profile"
            label="Select a profile"
            items={def.profiles.map((p) => p.name)}
            selectedItem={profile}
            onChange={({ selectedItem }) => { if (selectedItem) setProfile(selectedItem); }}
            itemToString={(name) => {
              const p = def.profiles.find((x) => x.name === name);
              return p ? `${p.name} — ${p.family}` : String(name ?? "");
            }}
          />
        </div>
        <Tabs>
          <TabList aria-label="Studio views">
            <Tab>Config layers</Tab>
            <Tab>Render review</Tab>
          </TabList>
          <TabPanels>
            <TabPanel>{profile && <ConfigLayers profile={profile} />}</TabPanel>
            <TabPanel>{profile && <RenderReview profile={profile} />}</TabPanel>
          </TabPanels>
        </Tabs>
      </main>
    </Theme>
  );
}
