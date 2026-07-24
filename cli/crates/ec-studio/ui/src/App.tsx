import { useEffect, useState } from "react";
import {
  Theme, Header, Dropdown, Tabs, TabList, Tab, TabPanels, TabPanel,
  Loading, InlineNotification, Tag,
} from "@carbon/react";
import { api, type DefinitionView } from "./api";
import { ConfigLayers } from "./ConfigLayers";
import { RenderReview } from "./RenderReview";
import { EvidenceReview } from "./EvidenceReview";
import { AccessControl } from "./AccessControl";
// The canonical EdgeCommons horizontal lockup, reversed for the dark app bar — the same asset the
// edge-console app bar uses.
import logoUrl from "./assets/edgecommons-logo-horizontal-reversed.svg";

export function App() {
  const [def, setDef] = useState<DefinitionView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [profile, setProfile] = useState<string>("");

  useEffect(() => {
    api.definition()
      .then((d) => { setDef(d); if (d.profiles[0]) setProfile(d.profiles[0].name); })
      .catch((e: Error) => setError(e.message));
  }, []);

  return (
    <Theme theme="g100" className="ec-app ec-app--dark">
      <Header aria-label="EdgeCommons Deployment Studio" className="ec-appbar">
        <a className="ec-appbar__brand" href="#" aria-label="EdgeCommons home">
          <img className="ec-appbar__logo" src={logoUrl} alt="EdgeCommons" />
        </a>
        <span className="ec-appbar__product">Deployment Studio</span>
      </Header>

      {error ? (
        <main className="ec-content">
          <InlineNotification kind="error" title="Cannot reach the Studio server" subtitle={error} hideCloseButton />
        </main>
      ) : !def ? (
        <Loading />
      ) : (
        <main className="ec-content">
          <div style={{ display: "flex", alignItems: "baseline", gap: "0.75rem", marginBottom: "0.25rem" }}>
            <h1>{def.name}</h1>
            <Tag type="cool-gray">read-only</Tag>
          </div>
          {def.description && <p style={{ marginTop: 0 }}>{def.description}</p>}
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
              <Tab>Evidence</Tab>
              <Tab>Access</Tab>
            </TabList>
            <TabPanels>
              <TabPanel>{profile && <ConfigLayers profile={profile} />}</TabPanel>
              <TabPanel>{profile && <RenderReview profile={profile} />}</TabPanel>
              <TabPanel>{profile && <EvidenceReview profile={profile} />}</TabPanel>
              <TabPanel><AccessControl /></TabPanel>
            </TabPanels>
          </Tabs>
        </main>
      )}
    </Theme>
  );
}
