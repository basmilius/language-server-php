//! The service container: `app('cache')` and `$this->app->make('db')` give the class the container
//! holds under a name the framework or a service provider registered.

use php_index::Type;
use php_index::framework::container;
use php_index::framework::overlay::{Marker, markers_for};

use crate::infer::{Analyzer, Arg, ResolvedCallable, literal_string};

impl Analyzer<'_> {
    /// Refines what a call to a container's `make` gives when the declared type could not say: the
    /// name in the argument is looked up among the bindings of the project and the framework.
    pub(crate) fn container_call_type(
        &self,
        callees: &[ResolvedCallable],
        args: &[Arg],
        env: &crate::infer::Env,
        result: Type,
    ) -> Type {
        if !self.index.frameworks().any() {
            return result;
        }
        if let Some(user) = self.user_call_type(callees, &result) {
            return user;
        }
        if let Some(repository) = self.repository_call_type(callees, args, env, &result) {
            return repository;
        }
        let unspecific = result
            .members()
            .iter()
            .all(|member| matches!(member, Type::Mixed | Type::Unknown | Type::Object | Type::Null));
        if !unspecific {
            return result;
        }
        for callee in callees {
            let (class, method) = match callee.name.split_once("::") {
                Some((class, method)) => (Some(class), method),
                None => (None, callee.name.as_str()),
            };
            for marker in markers_for(self.index, class, None, method) {
                let Marker::Container { position } = marker else {
                    continue;
                };
                let Some(name) = args
                    .get(position)
                    .and_then(|arg| arg.expr.as_ref())
                    .and_then(literal_string)
                else {
                    continue;
                };
                if let Some(found) = container::type_of(self.index, &name) {
                    return found;
                }
            }
        }
        result
    }
}

impl Analyzer<'_> {
    /// `Auth::user()` and `$request->user()` give the user model the application configures, where
    /// the framework can only say `Authenticatable` or `mixed`.
    fn user_call_type(&self, callees: &[ResolvedCallable], result: &Type) -> Option<Type> {
        let is_user_call = callees.iter().any(|callee| {
            let (class, method) = match callee.name.split_once("::") {
                Some((class, method)) => (Some(class), method),
                None => (None, callee.name.as_str()),
            };
            let receiver = callee
                .receiver
                .as_ref()
                .and_then(|ty| ty.class_names().first().map(|name| name.to_string()));
            markers_for(self.index, class, receiver.as_deref(), method).contains(&Marker::User)
        });
        if !is_user_call {
            return None;
        }
        let model = php_index::framework::auth::UserModel::class_of(self.index)?;
        let generic = matches!(result, Type::Mixed | Type::Unknown)
            || result
                .members()
                .iter()
                .any(|member| matches!(member, Type::Class { name, .. } if name.ends_with("Authenticatable")));
        generic.then(|| Type::union([Type::class(model), Type::Null]))
    }
}

impl Analyzer<'_> {
    /// `$entityManager->getRepository(User::class)` is the repository the entity names, where the
    /// declared type is only `EntityRepository<User>`.
    fn repository_call_type(
        &self,
        callees: &[ResolvedCallable],
        args: &[Arg],
        env: &crate::infer::Env,
        result: &Type,
    ) -> Option<Type> {
        if !self.index.frameworks().doctrine {
            return None;
        }
        let is_repository_call = callees.iter().any(|callee| {
            let (class, method) = match callee.name.split_once("::") {
                Some((class, method)) => (Some(class), method),
                None => (None, callee.name.as_str()),
            };
            let receiver = callee
                .receiver
                .as_ref()
                .and_then(|ty| ty.class_names().first().map(|name| name.to_string()));
            markers_for(self.index, class, receiver.as_deref(), method).contains(&Marker::Repository)
        });
        if !is_repository_call {
            return None;
        }
        let Type::ClassString(Some(entity)) = self.type_of(args.first()?.expr.as_ref()?, env) else {
            return None;
        };
        let Type::Class { name, .. } = *entity else {
            return None;
        };
        let repository = php_index::framework::symfony::doctrine::repository_of(self.index, &name)?;
        let _ = result;
        Some(Type::class(repository))
    }
}
