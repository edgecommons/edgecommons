# ggcommons-java-lib
  
#### Getting started

```
cd existing_repo
git remote add origin https://gitlab.aws.dev/greengrass-commons/ggcommons-java-lib.git
git branch -M main
git push -uf origin main
```

## Overview

Greengrass commons is a java based library that is aimed to solve configuration updates for one or more components deployed
across multiple devices.

Possible configuration options
1. **Environment** 
Good for development and local testing but does not scale well for industrial settings
2. **File**
Embedding unique configuration in the deployment configuration, while possible, removes the ability to do binary updates to multiple devices.
3. **Shadow**
Has core limitations:
 * 7 layers of json nesting
 * 8K size limitation (really 4K when “desired”/”reported” taken into account)
4. **Greengrass native**
Deployment configuration
Industrial use cases require unique component configuration per device. 
5. **gg component config** 
Deply GGCommonsComponent ( for “static” config and separating out unique configuration enables the use of GG deployment for updating component versions at scale
* Configs are stored in S3, one per component + common/site config
* Component pulls configs and caches locally
* Other components retrieve their configuration via IPC from the configuration manager
      
![img_1.png](img_1.png)



## Test and Deploy

Use the built-in continuous integration in GitLab.

- [ ] [Get started with GitLab CI/CD](https://docs.gitlab.com/ee/ci/quick_start/index.html)

***


## Name
Choose a self-explaining name for your project.

## Description
Let people know what your project can do specifically. Provide context and add a link to any reference visitors might be unfamiliar with. A list of Features or a Background subsection can also be added here. If there are alternatives to your project, this is a good place to list differentiating factors.



## Usage


## Support


## Roadmap


## Contributing


## Authors and acknowledgment
