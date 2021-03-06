use std::collections::{HashMap};

use ldap3::{Scope, LdapConnAsync, ResultEntry, SearchResult, SearchEntry, SearchOptions, Ldap};
use ldap3::result::{LdapError};

use crate::my_types::*;
use crate::my_err::{Result, MyErr};
use crate::ldap_filter;


pub type LdapAttrs = HashMap<String, Vec<String>>;

pub struct LdapW<'a> {
    pub ldap: Ldap,
    pub config: &'a LdapConfig,
    pub logged_user: &'a LoggedUser,
}

fn handle_read_one_search_result(res : SearchResult) -> Result<Option<ResultEntry>> {
    if res.1.rc == 0 {
        let mut l = res.0;
        Ok(l.pop())
    } else if res.1.rc == 32 /* NoSuchObject */ {
        Ok(None)
    } else {
        Err(MyErr::Ldap(LdapError::from(res.1)))
    }
}

fn search_options(sizelimit: Option<i32>) -> SearchOptions {
    let opts = SearchOptions::new();
    if let Some(limit) = sizelimit { opts.sizelimit(limit) } else { opts }
}

pub fn handle_sizelimited_search(res : SearchResult) -> Result<Vec<ResultEntry>> {
    if res.1.rc == 0 || res.1.rc == 4 {
        Ok(res.0)
    } else {
        Err(MyErr::Ldap(LdapError::from(res.1)))
    }
}

impl LdapW<'_> {
    pub async fn open_<'a>(cfg_and_lu: &'a CfgAndLU<'a>) -> Result<LdapW<'a>> {
        Self::open(&cfg_and_lu.cfg.ldap, &cfg_and_lu.user).await
    }

    pub async fn open<'a>(config: &'a LdapConfig, logged_user: &'a LoggedUser) -> Result<LdapW<'a>> {
        let (conn, mut ldap) = LdapConnAsync::new(&config.url).await?;
        ldap3::drive!(conn);
        ldap.simple_bind(&config.bind_dn.0, &config.bind_password).await?;
        Ok(LdapW { ldap, config, logged_user })
    }

    pub async fn search_raw<'a, S: AsRef<str> + Send + Sync + 'a>(
        &mut self, base: &str, filter: &str, attrs: Vec<S>, sizelimit: Option<i32>
    ) -> Result<Vec<ResultEntry>> {
        Ok(handle_sizelimited_search(self.ldap.with_search_options(search_options(sizelimit))
            .search(base, Scope::Subtree, filter, attrs).await?)?)
    }

    /*pub async fn search<'a, S: AsRef<str> + Send + Sync + 'a>(
        &mut self, base: &str, filter: &str, attrs: Vec<S>, sizelimit: Option<i32>
    ) -> Result<Vec<SearchEntry>> {
        let res = self.search_raw(base, filter, attrs, sizelimit).await?;
        Ok(res.into_iter().map(SearchEntry::construct).collect())
    }*/

    /*pub async fn search_one<'a, S: AsRef<str> + Send + Sync + 'a>(
        &mut self, base: &str, filter: &str, attrs: Vec<S>
    ) -> Result<Option<SearchEntry>> {
        let mut res = self.search_raw(base, filter, attrs, Some(1)).await?;
        Ok(res.pop().map(SearchEntry::construct))
    }*/

    pub async fn read<'a, S: AsRef<str> + Send + Sync + 'a>(&mut self, dn: &Dn, attrs: Vec<S>) -> Result<Option<SearchEntry>> {
        let res = self.ldap.search(&dn.0, Scope::Base, ldap_filter::true_(), attrs).await?;
        let res = handle_read_one_search_result(res)?;
        Ok(res.map(SearchEntry::construct))
    }

    pub async fn read_one_multi_attr(&mut self, dn: &Dn, attr: &str) -> Result<Option<Vec<String>>> {
        let entry = self.read(dn, vec![attr]).await?;
        Ok(entry.map(|e| get_all_values(e.attrs)))
    }

    #[allow(non_snake_case)]
    pub async fn read_one_multi_attr__or_err(&mut self, dn: &Dn, attr: &str) -> Result<Vec<String>> {
        self.read_one_multi_attr(dn, attr).await?.ok_or_else(
            || MyErr::Msg(format!("internal error (read_one_multi_attr__or_err expects {:?} to exist)", dn))
        )
    }

    pub async fn read_flattened_mright_raw(&mut self, dn: &Dn, mright: Mright) -> Result<Vec<Dn>> {
        let l = self.read_one_multi_attr__or_err(dn, self.config.to_flattened_attr(mright)).await?;
        Ok(l.into_iter().map(|dn| Dn(dn)).collect())
    }

    pub async fn read_flattened_mright(&mut self, dn: &Dn, mright: Mright) -> Result<Vec<Dn>> {
        let l = self.read_flattened_mright_raw(dn, mright).await?;
        // turn [""] into []
        Ok(match l.first() {
            Some(s) if s.0.is_empty() => vec![],
            _ => l
        })
    }

    pub async fn is_dn_matching_filter(&mut self, dn: &Dn, filter: &str) -> Result<bool> {
        let res = self.ldap.search(&dn.0, Scope::Base, dbg!(filter), vec![""]).await?;
        let res = handle_read_one_search_result(res)?;
        Ok(res.is_some())
    }

    pub async fn is_dn_existing(&mut self, dn: &Dn) -> Result<bool> {
        self.is_dn_matching_filter(dn, ldap_filter::true_()).await
    }

    pub async fn one_group_matches_filter(&mut self, filter: &str) -> Result<bool> {
        let rs = self.search_raw(&self.config.groups_dn.0, dbg!(filter), vec![""], Some(1)).await?;
        Ok(!rs.is_empty())
    }

    /*pub async fn search_one_mono_attr(&mut self, base: &str, filter: &str, attr: &str) -> Result<Vec<String>> {
        let (rs, _res) = self.ldap.search(base, Scope::Subtree, filter, vec![attr]).await?.success()?;
        Ok(rs.into_iter().filter_map(|r| result_entry_to_mono_attr(r, attr)).collect())
    }*/
}

/*fn result_entry_to_mono_attr(r: ldap3::ResultEntry, attr: &str) -> Option<String> {
    let attrs = &mut SearchEntry::construct(r).attrs;
    attrs.remove(attr)?.pop()
}*/

pub fn get_all_values(map: LdapAttrs) -> Vec<String> {
    map.into_values().flatten().collect()
}

pub fn mono_attrs(attrs: LdapAttrs) -> MonoAttrs {
    attrs.into_iter().filter_map(|(attr, val)| {
        let one = val.into_iter().next()?;
        Some((attr, one))
    }).collect()
}