// fauxx-desktop: Fauxx Desktop Companion
// Copyright (C) 2026 Digital Grease
//
// This program is free software: you can redistribute it and/or modify it
// under the terms of the GNU Affero General Public License as published by the
// Free Software Foundation, either version 3 of the License, or (at your
// option) any later version.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more
// details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

// Bundled, curated category -> representative-HTTPS-site table, kept in lockstep
// with crates/fauxx-core/src/browser/category_sites.json. This is the FALLBACK
// the extension uses when it is running standalone (no Core / native host
// connected): it lets the operator still drive a persona-category-aligned decoy
// session entirely offline. When the native host is connected, the host's
// decoy plan (see PROTOCOL.md) is authoritative and supersedes this table.
//
// Every URL is plain HTTPS, none require auth, and none are on the R3 sign-in
// blocklist (guardrails.isDecoyEligible enforces this at use time regardless).
export const CATEGORY_SITES = {
  MEDICAL: [
    "https://www.mayoclinic.org/",
    "https://medlineplus.gov/",
    "https://www.webmd.com/",
  ],
  LEGAL: [
    "https://www.law.cornell.edu/",
    "https://www.nolo.com/",
    "https://www.findlaw.com/",
  ],
  AUTOMOTIVE: [
    "https://www.caranddriver.com/",
    "https://www.motortrend.com/",
    "https://www.edmunds.com/",
  ],
  PARENTING: [
    "https://www.healthychildren.org/",
    "https://www.parents.com/",
    "https://www.zerotothree.org/",
  ],
  RETIREMENT: [
    "https://www.aarp.org/",
    "https://www.ssa.gov/",
    "https://www.investopedia.com/retirement-4427776",
  ],
  GAMING: [
    "https://www.ign.com/",
    "https://www.polygon.com/",
    "https://www.eurogamer.net/",
  ],
  AGRICULTURE: [
    "https://www.nass.usda.gov/",
    "https://www.farmprogress.com/",
    "https://www.agriculture.com/",
  ],
  FASHION: [
    "https://www.vogue.com/",
    "https://www.gq.com/",
    "https://www.harpersbazaar.com/",
  ],
  ACADEMIC: [
    "https://scholar.google.com/",
    "https://www.jstor.org/",
    "https://arxiv.org/",
  ],
  REAL_ESTATE: [
    "https://www.realtor.com/",
    "https://www.zillow.com/",
    "https://www.redfin.com/",
  ],
  COOKING: [
    "https://www.allrecipes.com/",
    "https://www.seriouseats.com/",
    "https://www.bonappetit.com/",
  ],
  SPORTS: [
    "https://www.espn.com/",
    "https://www.bbc.com/sport",
    "https://www.cbssports.com/",
  ],
  FINANCE: [
    "https://www.investopedia.com/",
    "https://www.bloomberg.com/",
    "https://www.morningstar.com/",
  ],
  TRAVEL: [
    "https://www.lonelyplanet.com/",
    "https://www.tripadvisor.com/",
    "https://www.nationalgeographic.com/travel",
  ],
  TECHNOLOGY: [
    "https://www.theverge.com/",
    "https://arstechnica.com/",
    "https://techcrunch.com/",
  ],
  PETS: [
    "https://www.akc.org/",
    "https://www.aspca.org/",
    "https://www.petmd.com/",
  ],
  HOME_IMPROVEMENT: [
    "https://www.thisoldhouse.com/",
    "https://www.familyhandyman.com/",
    "https://www.bobvila.com/",
  ],
  BEAUTY: [
    "https://www.allure.com/",
    "https://www.byrdie.com/",
    "https://www.sephora.com/",
  ],
  MUSIC: [
    "https://pitchfork.com/",
    "https://www.rollingstone.com/music/",
    "https://www.billboard.com/",
  ],
  FITNESS: [
    "https://www.menshealth.com/",
    "https://www.self.com/",
    "https://www.acefitness.org/",
  ],
  ENTERTAINMENT: [
    "https://variety.com/",
    "https://www.hollywoodreporter.com/",
    "https://www.imdb.com/",
  ],
  FOOD: [
    "https://www.foodnetwork.com/",
    "https://www.epicurious.com/",
    "https://www.eater.com/",
  ],
  POLITICS: [
    "https://www.reuters.com/world/",
    "https://apnews.com/hub/politics",
    "https://www.bbc.com/news",
  ],
  SCIENCE: [
    "https://www.scientificamerican.com/",
    "https://www.nature.com/",
    "https://www.sciencenews.org/",
  ],
  BUSINESS: [
    "https://www.forbes.com/",
    "https://hbr.org/",
    "https://www.economist.com/",
  ],
  OUTDOOR_RECREATION: [
    "https://www.rei.com/",
    "https://www.outsideonline.com/",
    "https://www.alltrails.com/",
  ],
  CRAFTS: [
    "https://www.craftsy.com/",
    "https://www.instructables.com/",
    "https://www.michaels.com/",
  ],
  HISTORY: [
    "https://www.history.com/",
    "https://www.smithsonianmag.com/history/",
    "https://www.britannica.com/",
  ],
  ENVIRONMENT: [
    "https://www.nationalgeographic.com/environment",
    "https://www.epa.gov/",
    "https://e360.yale.edu/",
  ],
  MILITARY_DEFENSE: [
    "https://www.military.com/",
    "https://www.defensenews.com/",
    "https://www.janes.com/",
  ],
  WELLNESS_ALTERNATIVE: [
    "https://www.healthline.com/",
    "https://www.mindbodygreen.com/",
    "https://www.yogajournal.com/",
  ],
  RELATIONSHIPS_DATING: [
    "https://www.psychologytoday.com/us/basics/relationships",
    "https://www.gottman.com/",
    "https://www.brides.com/",
  ],
};

// Resolve an ordered, de-duplicated list of HTTPS sites for an explicit set of
// CategoryPool category names. Unknown category names are skipped (they map to
// no curated sites), mirroring sites_for_categories() in the Rust core.
export function sitesForCategories(categories) {
  const seen = new Set();
  const out = [];
  for (const category of categories || []) {
    const sites = CATEGORY_SITES[category];
    if (!sites) {
      continue;
    }
    for (const site of sites) {
      if (!seen.has(site)) {
        seen.add(site);
        out.push(site);
      }
    }
  }
  return out;
}
