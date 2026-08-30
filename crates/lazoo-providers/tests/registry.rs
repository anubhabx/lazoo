use lazoo_providers::{DiscoveryEnvironment, ProviderKind, ProviderRegistry};

#[test]
fn registry_lists_every_provider_once_in_stable_order() {
    let discoveries = ProviderRegistry::default().discover(&DiscoveryEnvironment::empty());

    assert_eq!(discoveries.len(), ProviderKind::ALL.len());
    assert_eq!(
        discoveries
            .iter()
            .map(|discovery| discovery.provider)
            .collect::<Vec<_>>(),
        ProviderKind::ALL
    );
    assert!(
        discoveries
            .iter()
            .all(|discovery| discovery.candidate.is_none())
    );
}
