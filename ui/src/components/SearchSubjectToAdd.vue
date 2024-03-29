<script lang="ts">
import { computed, ref } from "vue";
import { asyncComputed } from "@vueuse/core";
import * as api from '@/api'

const noResultsMsg = "Aucun résultat"
const default_moreResultsMsg = (limit: number) => (
    `Votre recherche est limitée à ${limit} résultats.<br>Pour les autres résultats veuillez affiner la recherche.`
)

const search_subjects = async (ldapCfg: LdapConfigOut, search_token: string, sizelimit: number) => {
    const r = await api.search_subjects({ search_token, sizelimit })
    forEach(r, (subjects, ssdn) => {
        const sscfg = ldapCfg.subject_sources.find(sscfg => sscfg.dn === ssdn);
        if (sscfg) {
            r[ssdn] = objectSortBy(subjects, (subject, _) => at(subject.attrs, sscfg.display_attrs).join(';'))
        }
    })
    return r
}

</script>

<script setup lang="ts">
import { at, isEmpty, size } from 'lodash'
import { forEach, objectSortBy, some } from "@/helpers";
import { vFocus } from '@/vue_helpers';
import SubjectOrGroup from "./SubjectOrGroup.vue";
import MyIcon from "./MyIcon.vue";
import { LdapConfigOut, PRecord, Dn, Subjects } from "@/my_types";

let ldapCfg = asyncComputed(api.config_ldap)

interface Props {
    minChars?: number
    limit?: number
    placeholder?: string
}

let props = withDefaults(defineProps<Props>(), {
    minChars: 3,
    limit: 10,
})

let moreResultsMsg_ = computed(() => default_moreResultsMsg(props.limit))

let loading = ref(false)
let query = ref('')
let items = ref({} as PRecord<Dn, Subjects>)
let noResults = ref(false)
let moreResults = ref(false)
let current = ref(0)
let cancel = ref((_?: unknown) => {})

function open() {
    cancel.value()

    if (!ldapCfg.value) return

    if (props.minChars && (!query.value || query.value.length < props.minChars)) {
        stopAndClose()
        return
    }

    setTimeout(() => {
        loading.value = true
        Promise.race([
            new Promise((resolve) => cancel.value = resolve),
            search_subjects(ldapCfg.value, query.value, props.limit+1),
        ]).then((data) => {
            if (!data) return; // canceled
            setOptions(data as PRecord<Dn, Subjects>)
        })
    }, 500)
}

function setOptions(data: PRecord<Dn, Subjects>) {
    current.value = 0
    items.value = data
    moreResults.value = some(data, (subjects, _) => (
        size(subjects) > props.limit
    ))
    noResults.value = isEmpty(items)
    loading.value = false
}

function stopAndClose() {
    console.log("stopAndClose")
    cancel.value()
    items.value = {}
    noResults.value = false
    loading.value = false
}

</script>

<template>
    <div>
        <input :aria-label="placeholder" :placeholder="placeholder"
           v-model="query" v-focus
           type="text" autocomplete="off"
           @keydown.esc="stopAndClose"
           @blur="stopAndClose"
           @focus="open"
           @input="open">
        <MyIcon class="fa-spin end-of-input" name="spinner" v-if="loading"/>
   </div>
<div class="popup" v-if="!isEmpty(items) || noResults">
    <table>
        <tr v-if="moreResults" class="moreResultsMsg"><td v-html="moreResultsMsg_"></td></tr>
        <tr v-if="moreResults" role="separator" class="divider"></tr>
        <tr v-if="noResults"><td>{{noResultsMsg}}</td></tr>
        <template v-for="(subjects, ssdn) in items">
            <template v-if="!isEmpty(subjects)">
                <thead class="ss_name">{{ldapCfg?.subject_sources.find(sscfg => sscfg.dn === ssdn)?.name}}</thead>
                <tbody>
                    <tr v-for="(subject, dn) in subjects" 
                        @mousedown.prevent=""> <!-- do not blur "input" -->
                        <td><SubjectOrGroup :dn="dn" :subject="subject" :ssdn="ssdn" /></td>
                        <td><slot :close="stopAndClose" :dn="dn" /></td>
                    </tr>
                </tbody>
            </template>
        </template>
    </table>
</div>
</template>

<style scoped>
table tr.divider {
  height: 2px;
  background: #aaa;
}

input {
    padding-right: 18px;
    width: 100%;
    box-sizing: border-box;
}
.my-icon.end-of-input {
  margin-left: -18px;
}
</style>
